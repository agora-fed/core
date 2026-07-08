// Cliente Web Push — registra o service worker, pede permissão, cria a
// subscription com a chave VAPID pública do back, e envia pro
// POST /me/push-subscriptions.
//
// Retornos são objetos simples pra UI decidir mostrar "ativado", "desativado"
// ou "erro". Usa só APIs padrão do navegador (PushManager + Notification),
// sem depender de biblioteca externa.
import { getVapidPublicKey, subscribeWebPush } from './api';

export type PushStatus = 'unsupported' | 'denied' | 'default' | 'granted';

function urlBase64ToUint8Array(base64: string): Uint8Array {
  const padding = '='.repeat((4 - (base64.length % 4)) % 4);
  const b64 = (base64 + padding).replace(/-/g, '+').replace(/_/g, '/');
  const raw = atob(b64);
  const arr = new Uint8Array(raw.length);
  for (let i = 0; i < raw.length; i++) arr[i] = raw.charCodeAt(i);
  return arr;
}

export function pushStatus(): PushStatus {
  if (typeof window === 'undefined') return 'unsupported';
  if (!('serviceWorker' in navigator) || !('PushManager' in window)) return 'unsupported';
  return Notification.permission as PushStatus;
}

/** Registra o SW se ainda não estiver registrado. Retorna o registration. */
async function ensureServiceWorker(): Promise<ServiceWorkerRegistration> {
  const existing = await navigator.serviceWorker.getRegistration('/sw.js');
  if (existing) return existing;
  return navigator.serviceWorker.register('/sw.js', { scope: '/' });
}

/**
 * Sequência completa: SW ativo → permissão → busca VAPID pub key → subscribe
 * no PushManager → POST no back. Retorna true em sucesso, false com console
 * detalhando o motivo.
 */
export async function enablePush(): Promise<{ ok: boolean; reason?: string }> {
  if (pushStatus() === 'unsupported') {
    return { ok: false, reason: 'Este navegador não suporta notificações push.' };
  }
  const perm = await Notification.requestPermission();
  if (perm !== 'granted') {
    return { ok: false, reason: 'Permissão de notificação negada.' };
  }
  const vapid = await getVapidPublicKey();
  if (!vapid.success || !vapid.data?.public_key) {
    return {
      ok: false,
      reason: 'A instância ainda não configurou VAPID — avise o operador.',
    };
  }
  const registration = await ensureServiceWorker();
  await navigator.serviceWorker.ready;
  const applicationServerKey = urlBase64ToUint8Array(vapid.data.public_key);
  const sub = await registration.pushManager.subscribe({
    userVisibleOnly: true,
    applicationServerKey,
  });
  const json = sub.toJSON() as PushSubscriptionJSON;
  const res = await subscribeWebPush(json, navigator.userAgent);
  if (!res.success) {
    return { ok: false, reason: res.error?.message ?? 'Falha ao registrar no servidor.' };
  }
  return { ok: true };
}

/** Cancela a subscription local (não deleta do back — TTL cuida via 410). */
export async function disablePush(): Promise<void> {
  if (pushStatus() === 'unsupported') return;
  const reg = await navigator.serviceWorker.getRegistration('/sw.js');
  if (!reg) return;
  const sub = await reg.pushManager.getSubscription();
  if (sub) await sub.unsubscribe();
}

/** Já está inscrito nesta origem? */
export async function isSubscribed(): Promise<boolean> {
  if (pushStatus() === 'unsupported') return false;
  const reg = await navigator.serviceWorker.getRegistration('/sw.js');
  if (!reg) return false;
  const sub = await reg.pushManager.getSubscription();
  return !!sub;
}
