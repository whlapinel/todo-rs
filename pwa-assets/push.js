// Push notification subscribe/unsubscribe flow (docs/push-notifications-plan.md). Copied
// into static/ by styles/package.json's existing pwa-assets build step (same as sw.js).
(function () {
  function isIosSafari() {
    return /iP(hone|ad|od)/.test(navigator.userAgent) && !window.MSStream;
  }

  function isStandalone() {
    return (
      window.matchMedia("(display-mode: standalone)").matches ||
      window.navigator.standalone === true
    );
  }

  function urlBase64ToUint8Array(base64Url) {
    const padding = "=".repeat((4 - (base64Url.length % 4)) % 4);
    const base64 = (base64Url + padding).replace(/-/g, "+").replace(/_/g, "/");
    const raw = window.atob(base64);
    return Uint8Array.from([...raw].map((c) => c.charCodeAt(0)));
  }

  async function refreshToggleState(toggle, hint) {
    if (!("serviceWorker" in navigator) || !("PushManager" in window)) {
      toggle.hidden = true;
      return;
    }
    if (isIosSafari() && !isStandalone()) {
      toggle.hidden = true;
      hint.hidden = false;
      return;
    }
    const registration = await navigator.serviceWorker.ready;
    const subscription = await registration.pushManager.getSubscription();
    toggle.checked = !!subscription;
    toggle.hidden = false;
  }

  async function enable(toggle) {
    const keyResponse = await fetch("/web/push/public-key");
    if (!keyResponse.ok) {
      toggle.checked = false;
      toggle.hidden = true;
      return;
    }
    const publicKey = await keyResponse.text();

    const permission = await Notification.requestPermission();
    if (permission !== "granted") {
      toggle.checked = false;
      return;
    }

    const registration = await navigator.serviceWorker.ready;
    const subscription = await registration.pushManager.subscribe({
      userVisibleOnly: true,
      applicationServerKey: urlBase64ToUint8Array(publicKey),
    });
    const json = subscription.toJSON();
    await fetch("/web/push/subscribe", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ endpoint: json.endpoint, keys: json.keys }),
    });
  }

  async function disable(toggle) {
    const registration = await navigator.serviceWorker.ready;
    const subscription = await registration.pushManager.getSubscription();
    if (!subscription) return;
    const endpoint = subscription.endpoint;
    await subscription.unsubscribe();
    await fetch("/web/push/unsubscribe", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ endpoint }),
    });
  }

  function wireToggle() {
    const toggle = document.getElementById("push-notif-toggle");
    const hint = document.getElementById("push-notif-ios-hint");
    if (!toggle || toggle.dataset.pushWired) return;
    toggle.dataset.pushWired = "1";

    refreshToggleState(toggle, hint);
    toggle.addEventListener("change", () => {
      if (toggle.checked) {
        enable(toggle);
      } else {
        disable(toggle);
      }
    });
  }

  // The bell dropdown fragment (templates/notifications/list.html) is swapped in by htmx
  // after the initial page load, so the toggle doesn't exist yet at DOMContentLoaded —
  // re-wire on every htmx swap the same way any other post-swap JS in this app would.
  document.addEventListener("DOMContentLoaded", wireToggle);
  document.body.addEventListener("htmx:afterSwap", wireToggle);
})();
