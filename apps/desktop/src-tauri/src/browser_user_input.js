(() => {
  const send = window.ipc.postMessage.bind(window.ipc);
  let last = 0;
  const heard = (event) => {
    if (!event.isTrusted) return;
    const now = Date.now();
    if (now - last < 1000) return;
    last = now;
    send("browser:user-input");
  };
  for (const kind of ["pointerdown", "keydown", "wheel"]) {
    window.addEventListener(kind, heard, { capture: true, passive: true });
  }
})();
