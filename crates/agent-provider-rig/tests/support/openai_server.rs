use std::{sync::Arc, time::Duration};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Mutex,
    task::JoinHandle,
    time::timeout,
};

const MAX_REQUESTS: usize = 8;
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 64 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(3);

pub struct ScriptedResponse {
    status: u16,
    body: String,
}

impl ScriptedResponse {
    pub fn json(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
        }
    }
}

pub struct CapturedRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl CapturedRequest {
    pub fn method(&self) -> &str {
        &self.method
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn json_body(&self) -> Result<rig_core::serde_json::Value, String> {
        rig_core::serde_json::from_slice(&self.body)
            .map_err(|_| "captured request body was not valid JSON".to_owned())
    }
}

pub struct TestOpenAiServer {
    base_url: String,
    captures: Arc<Mutex<Vec<CapturedRequest>>>,
    task: JoinHandle<Result<(), String>>,
}

impl TestOpenAiServer {
    pub async fn start(responses: Vec<ScriptedResponse>) -> Result<Self, String> {
        if responses.is_empty() || responses.len() > MAX_REQUESTS {
            return Err("test server requires a small non-empty response script".to_owned());
        }

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|_| "test server failed to bind loopback".to_owned())?;
        let address = listener
            .local_addr()
            .map_err(|_| "test server failed to inspect its address".to_owned())?;
        let captures = Arc::new(Mutex::new(Vec::with_capacity(responses.len())));
        let task_captures = Arc::clone(&captures);
        let task = tokio::spawn(async move {
            for response in responses {
                let (stream, _) = timeout(IO_TIMEOUT, listener.accept())
                    .await
                    .map_err(|_| "test server timed out waiting for a request".to_owned())?
                    .map_err(|_| "test server failed to accept a request".to_owned())?;
                let request = read_request(stream, response).await?;
                task_captures.lock().await.push(request);
            }
            Ok(())
        });

        Ok(Self {
            base_url: format!("http://{address}/v1"),
            captures,
            task,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn base_url_with_trailing_slash(&self) -> String {
        format!("{}/", self.base_url)
    }

    pub async fn finish(self) -> Result<Vec<CapturedRequest>, String> {
        self.task
            .await
            .map_err(|_| "test server task failed".to_owned())??;
        let mut captures = self.captures.lock().await;
        Ok(std::mem::take(&mut *captures))
    }
}

async fn read_request(
    mut stream: TcpStream,
    response: ScriptedResponse,
) -> Result<CapturedRequest, String> {
    let mut received = Vec::new();
    let header_end = loop {
        if received.len() >= MAX_HEADER_BYTES {
            return Err("test request headers exceeded the configured bound".to_owned());
        }

        let mut chunk = [0_u8; 1024];
        let read = timeout(IO_TIMEOUT, stream.read(&mut chunk))
            .await
            .map_err(|_| "test server timed out reading request headers".to_owned())?
            .map_err(|_| "test server failed reading request headers".to_owned())?;
        if read == 0 {
            return Err("test client closed before sending complete headers".to_owned());
        }
        received.extend_from_slice(&chunk[..read]);

        if let Some(position) = find_header_end(&received) {
            if position > MAX_HEADER_BYTES {
                return Err("test request headers exceeded the configured bound".to_owned());
            }
            break position;
        }
    };

    let header_bytes = &received[..header_end];
    let header_text = std::str::from_utf8(header_bytes)
        .map_err(|_| "test request headers were not UTF-8".to_owned())?;
    let mut lines = header_text.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| "test request line was missing".to_owned())?;
    let mut request_parts = request_line.split(' ');
    let method = request_parts
        .next()
        .ok_or_else(|| "test request method was missing".to_owned())?;
    let path = request_parts
        .next()
        .ok_or_else(|| "test request path was missing".to_owned())?;
    let version = request_parts
        .next()
        .ok_or_else(|| "test HTTP version was missing".to_owned())?;
    if request_parts.next().is_some() || version != "HTTP/1.1" {
        return Err("test server supports only a standard HTTP/1.1 request line".to_owned());
    }

    let mut headers = Vec::new();
    let mut content_length = None;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| "test request contained a malformed header".to_owned())?;
        let value = value.trim().to_owned();
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err("test server does not support transfer encoding".to_owned());
        }
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err("test request contained duplicate Content-Length".to_owned());
            }
            content_length = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| "test request Content-Length was invalid".to_owned())?,
            );
        }
        headers.push((name.to_owned(), value));
    }

    let content_length = content_length
        .ok_or_else(|| "test server accepts only Content-Length requests".to_owned())?;
    if content_length > MAX_BODY_BYTES {
        return Err("test request body exceeded the configured bound".to_owned());
    }

    let body_start = header_end + 4;
    let mut body = received[body_start..].to_vec();
    if body.len() > content_length {
        return Err("test request contained bytes after its declared body".to_owned());
    }
    while body.len() < content_length {
        let remaining = content_length - body.len();
        let mut chunk = vec![0_u8; remaining.min(4096)];
        let read = timeout(IO_TIMEOUT, stream.read(&mut chunk))
            .await
            .map_err(|_| "test server timed out reading request body".to_owned())?
            .map_err(|_| "test server failed reading request body".to_owned())?;
        if read == 0 {
            return Err("test client closed before sending its declared body".to_owned());
        }
        body.extend_from_slice(&chunk[..read]);
    }

    write_response(&mut stream, response).await?;

    Ok(CapturedRequest {
        method: method.to_owned(),
        path: path.to_owned(),
        headers,
        body,
    })
}

async fn write_response(stream: &mut TcpStream, response: ScriptedResponse) -> Result<(), String> {
    let reason = reason_phrase(response.status);
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.status,
        reason,
        response.body.len()
    );

    timeout(IO_TIMEOUT, stream.write_all(head.as_bytes()))
        .await
        .map_err(|_| "test server timed out writing response headers".to_owned())?
        .map_err(|_| "test server failed writing response headers".to_owned())?;
    timeout(IO_TIMEOUT, stream.write_all(response.body.as_bytes()))
        .await
        .map_err(|_| "test server timed out writing response body".to_owned())?
        .map_err(|_| "test server failed writing response body".to_owned())?;
    timeout(IO_TIMEOUT, stream.shutdown())
        .await
        .map_err(|_| "test server timed out closing the connection".to_owned())?
        .map_err(|_| "test server failed closing the connection".to_owned())?;
    Ok(())
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        408 => "Request Timeout",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Test Status",
    }
}
