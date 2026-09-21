import "@testing-library/jest-dom/vitest";

Object.defineProperty(window, "innerWidth", { writable: true, value: 1280 });

Object.defineProperty(window, "matchMedia", {
  writable: true,
  value: (query: string) => {
    const minWidth = /min-width:\s*(\d+)px/.exec(query)?.[1];
    return {
      matches: minWidth === undefined ? false : window.innerWidth >= Number(minWidth),
      media: query,
      onchange: null,
      addEventListener: () => undefined,
      removeEventListener: () => undefined,
      addListener: () => undefined,
      removeListener: () => undefined,
      dispatchEvent: () => false,
    };
  },
});
