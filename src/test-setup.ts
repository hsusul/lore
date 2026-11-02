// jsdom lacks layout and pointer-capture APIs that Radix and cmdk call. Node-environment
// test files (the theme and token checks) have no DOM, so the shims apply only under jsdom.
if (typeof Element !== "undefined") {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };

  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.releasePointerCapture ??= () => {};
  Element.prototype.scrollIntoView ??= () => {};
}
