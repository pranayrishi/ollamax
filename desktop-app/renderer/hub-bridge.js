// Hub bridge — the host side of the reused media/hub.js, but talking to the
// main process (which reads the account-server catalog + writes local rules/
// skills) over preload IPC instead of the VS Code extension host. Same protocol
// the extension's HubViewProvider spoke: in {ready, openPackage, activate,
// support}; out {categories, package, activated, needsServer, needsSignIn,
// error}. Starring stays OPT-IN ONLY (browser review), never automatic.
(function () {
  const post = (m) => window.postMessage(m, "*");
  const hub = () => (window.forgeNative && window.forgeNative.hub) || null;

  async function handle(msg) {
    const h = hub();
    switch (msg.type) {
      case "ready": {
        if (!h) return post({ type: "needsServer" });
        const r = await h.categories();
        if (r.needsServer) post({ type: "needsServer" });
        else if (r.error) post({ type: "error", message: r.error });
        else post({ type: "categories", categories: r.categories || [] });
        break;
      }
      case "openPackage": {
        if (!h) return;
        const r = await h.package(msg.slug);
        if (r.error) post({ type: "error", message: r.error });
        else if (r.pkg) post({ type: "package", pkg: r.pkg });
        break;
      }
      case "activate": {
        if (!h) return;
        const r = await h.activate(msg.slug);
        if (r.error) post({ type: "error", message: r.error });
        else if (r.activated) post({ type: "activated", slug: r.slug, name: r.name, counts: r.counts });
        break;
      }
      case "support": {
        if (!h) return;
        // The main process obtains any bearer token from encrypted account
        // storage itself. This renderer never receives or supplies one.
        const r = await h.support({ slug: msg.slug, repos: msg.repos });
        if (r.needsSignIn) post({ type: "needsSignIn" });
        else if (r.error) post({ type: "error", message: r.error });
        // r.ok → the browser opened for conscious, opt-in starring.
        break;
      }
      default:
        break; // chat messages — not ours
    }
  }

  window.__forgeRegisterBridge(handle);
})();
