//! Deterministic adapters for harness tests and the M1 CLI demonstration.
//!
//! This module is available only to crate tests or consumers enabling the
//! `test-support` feature.

use std::{
    collections::VecDeque,
    future::pending,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use agent_core::{AgentEvent, ModelRequest, ModelResponse, ToolCall, ToolDefinition, ToolResult};

use crate::{
    AuditPortError, AuditSink, ModelPort, ModelPortError, PortFuture, ToolPort, ToolPortError,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FakeInvocation {
    Model,
    Tool,
    Audit,
}

#[derive(Clone, Default)]
pub struct InvocationLog {
    entries: Arc<Mutex<Vec<FakeInvocation>>>,
}

impl InvocationLog {
    #[must_use]
    pub fn entries(&self) -> Vec<FakeInvocation> {
        lock_recover(&self.entries).clone()
    }

    fn push(&self, entry: FakeInvocation) {
        lock_recover(&self.entries).push(entry);
    }
}

#[derive(Clone)]
enum ModelBehavior {
    Immediate(Result<ModelResponse, ModelPortError>),
    Delayed(Duration, Result<ModelResponse, ModelPortError>),
    Pending,
}

pub struct FakeModelPort {
    behaviors: Mutex<VecDeque<ModelBehavior>>,
    invocations: AtomicUsize,
    log: InvocationLog,
}

impl FakeModelPort {
    #[must_use]
    pub fn scripted(results: Vec<Result<ModelResponse, ModelPortError>>) -> Self {
        Self::with_behaviors(
            results.into_iter().map(ModelBehavior::Immediate).collect(),
            InvocationLog::default(),
        )
    }

    #[must_use]
    pub fn delayed(delay: Duration, result: Result<ModelResponse, ModelPortError>) -> Self {
        Self::with_behaviors(
            VecDeque::from([ModelBehavior::Delayed(delay, result)]),
            InvocationLog::default(),
        )
    }

    #[must_use]
    pub fn pending() -> Self {
        Self::with_behaviors(
            VecDeque::from([ModelBehavior::Pending]),
            InvocationLog::default(),
        )
    }

    #[must_use]
    pub fn with_log(
        results: Vec<Result<ModelResponse, ModelPortError>>,
        log: InvocationLog,
    ) -> Self {
        Self::with_behaviors(
            results.into_iter().map(ModelBehavior::Immediate).collect(),
            log,
        )
    }

    fn with_behaviors(behaviors: VecDeque<ModelBehavior>, log: InvocationLog) -> Self {
        Self {
            behaviors: Mutex::new(behaviors),
            invocations: AtomicUsize::new(0),
            log,
        }
    }

    #[must_use]
    pub fn invocation_count(&self) -> usize {
        self.invocations.load(Ordering::SeqCst)
    }
}

impl ModelPort for FakeModelPort {
    fn invoke<'a>(
        &'a self,
        _request: ModelRequest,
    ) -> PortFuture<'a, Result<ModelResponse, ModelPortError>> {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        self.log.push(FakeInvocation::Model);
        let behavior = lock_recover(&self.behaviors)
            .pop_front()
            .unwrap_or(ModelBehavior::Immediate(Err(ModelPortError::Unavailable)));
        Box::pin(async move {
            match behavior {
                ModelBehavior::Immediate(result) => result,
                ModelBehavior::Delayed(delay, result) => {
                    tokio::time::sleep(delay).await;
                    result
                }
                ModelBehavior::Pending => pending().await,
            }
        })
    }
}

#[derive(Clone)]
enum ToolBehavior {
    Immediate(Result<ToolResult, ToolPortError>),
    Delayed(Duration, Result<ToolResult, ToolPortError>),
    Pending,
}

pub struct FakeToolPort {
    definition: ToolDefinition,
    behaviors: Mutex<VecDeque<ToolBehavior>>,
    invocations: AtomicUsize,
    log: InvocationLog,
}

impl FakeToolPort {
    #[must_use]
    pub fn scripted(
        definition: ToolDefinition,
        results: Vec<Result<ToolResult, ToolPortError>>,
    ) -> Self {
        Self::with_behaviors(
            definition,
            results.into_iter().map(ToolBehavior::Immediate).collect(),
            InvocationLog::default(),
        )
    }

    #[must_use]
    pub fn delayed(
        definition: ToolDefinition,
        delay: Duration,
        result: Result<ToolResult, ToolPortError>,
    ) -> Self {
        Self::with_behaviors(
            definition,
            VecDeque::from([ToolBehavior::Delayed(delay, result)]),
            InvocationLog::default(),
        )
    }

    #[must_use]
    pub fn pending(definition: ToolDefinition) -> Self {
        Self::with_behaviors(
            definition,
            VecDeque::from([ToolBehavior::Pending]),
            InvocationLog::default(),
        )
    }

    #[must_use]
    pub fn with_log(
        definition: ToolDefinition,
        results: Vec<Result<ToolResult, ToolPortError>>,
        log: InvocationLog,
    ) -> Self {
        Self::with_behaviors(
            definition,
            results.into_iter().map(ToolBehavior::Immediate).collect(),
            log,
        )
    }

    fn with_behaviors(
        definition: ToolDefinition,
        behaviors: VecDeque<ToolBehavior>,
        log: InvocationLog,
    ) -> Self {
        Self {
            definition,
            behaviors: Mutex::new(behaviors),
            invocations: AtomicUsize::new(0),
            log,
        }
    }

    #[must_use]
    pub fn invocation_count(&self) -> usize {
        self.invocations.load(Ordering::SeqCst)
    }
}

impl ToolPort for FakeToolPort {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn invoke<'a>(&'a self, _call: ToolCall) -> PortFuture<'a, Result<ToolResult, ToolPortError>> {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        self.log.push(FakeInvocation::Tool);
        let behavior = lock_recover(&self.behaviors)
            .pop_front()
            .unwrap_or(ToolBehavior::Immediate(Err(ToolPortError::Unavailable)));
        Box::pin(async move {
            match behavior {
                ToolBehavior::Immediate(result) => result,
                ToolBehavior::Delayed(delay, result) => {
                    tokio::time::sleep(delay).await;
                    result
                }
                ToolBehavior::Pending => pending().await,
            }
        })
    }
}

#[derive(Clone, Default)]
pub struct InMemoryAuditSink {
    events: Arc<Mutex<Vec<AgentEvent>>>,
    log: InvocationLog,
}

impl InMemoryAuditSink {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_log(log: InvocationLog) -> Self {
        Self {
            events: Arc::default(),
            log,
        }
    }

    #[must_use]
    pub fn events(&self) -> Vec<AgentEvent> {
        lock_recover(&self.events).clone()
    }
}

impl AuditSink for InMemoryAuditSink {
    fn record<'a>(&'a self, event: &'a AgentEvent) -> PortFuture<'a, Result<(), AuditPortError>> {
        self.log.push(FakeInvocation::Audit);
        lock_recover(&self.events).push(event.clone());
        Box::pin(std::future::ready(Ok(())))
    }
}

pub struct FailingAuditSink {
    fail_on_attempt: Option<usize>,
    attempts: AtomicUsize,
    recorded: Mutex<Vec<AgentEvent>>,
}

impl FailingAuditSink {
    #[must_use]
    pub fn always() -> Self {
        Self {
            fail_on_attempt: None,
            attempts: AtomicUsize::new(0),
            recorded: Mutex::default(),
        }
    }

    #[must_use]
    pub fn on_attempt(attempt: usize) -> Self {
        Self {
            fail_on_attempt: Some(attempt),
            attempts: AtomicUsize::new(0),
            recorded: Mutex::default(),
        }
    }

    #[must_use]
    pub fn attempt_count(&self) -> usize {
        self.attempts.load(Ordering::SeqCst)
    }

    #[must_use]
    pub fn recorded_events(&self) -> Vec<AgentEvent> {
        lock_recover(&self.recorded).clone()
    }
}

impl AuditSink for FailingAuditSink {
    fn record<'a>(&'a self, event: &'a AgentEvent) -> PortFuture<'a, Result<(), AuditPortError>> {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
        let fails = self
            .fail_on_attempt
            .is_none_or(|configured| configured == attempt);
        if fails {
            Box::pin(std::future::ready(Err(AuditPortError::RecordFailed)))
        } else {
            lock_recover(&self.recorded).push(event.clone());
            Box::pin(std::future::ready(Ok(())))
        }
    }
}

fn lock_recover<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
