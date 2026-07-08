// DemocraciaBR — service worker mínimo pra Web Push (RFC 8291).
//
// Só o `push` event importa aqui: o back manda um JSON `{title, body, url}`
// e a gente mostra a notificação nativa. Click abre a URL do payload.
//
// Nenhum cache, nenhum offline shell — o site continua sendo Astro SSG servido
// pelo gateway. Este worker é *só* pra push.

self.addEventListener('install', (event) => {
  event.waitUntil(self.skipWaiting());
});
self.addEventListener('activate', (event) => {
  event.waitUntil(self.clients.claim());
});

self.addEventListener('push', (event) => {
  if (!event.data) return;
  let data;
  try {
    data = event.data.json();
  } catch {
    data = { title: 'DemocraciaBR', body: event.data.text() };
  }
  const title = data.title || 'DemocraciaBR';
  const options = {
    body: data.body || '',
    icon: '/favicon-512.png',
    badge: '/favicon-512.png',
    data: { url: data.url || '/notificacoes' },
    // Vibração leve — só efeito em Android.
    vibrate: [80, 40, 80],
  };
  event.waitUntil(
    (async () => {
      await self.registration.showNotification(title, options);
      // Broadcast pras abas abertas atualizarem o badge do sino sem
      // esperar o poll de 60s.
      const clients = await self.clients.matchAll({
        type: 'window',
        includeUncontrolled: true,
      });
      for (const client of clients) {
        client.postMessage({ type: 'dsoc-push', payload: data });
      }
    })(),
  );
});

self.addEventListener('notificationclick', (event) => {
  event.notification.close();
  const url = (event.notification.data && event.notification.data.url) || '/';
  event.waitUntil(
    self.clients.matchAll({ type: 'window', includeUncontrolled: true }).then((clients) => {
      // Se já tem uma aba aberta na mesma origem, foca ela e navega.
      for (const client of clients) {
        if ('focus' in client) {
          client.focus();
          if ('navigate' in client) client.navigate(url);
          return;
        }
      }
      if (self.clients.openWindow) return self.clients.openWindow(url);
    }),
  );
});
