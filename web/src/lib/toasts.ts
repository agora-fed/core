// Minimal toast store. Any island can call `toast.success(...)` / .error /
// .info; a single <ToastHost/> mounted in AppShell renders the queue.
import { writable } from 'svelte/store';

export type ToastTone = 'success' | 'error' | 'info' | 'warning';

export interface Toast {
  id: string;
  tone: ToastTone;
  title?: string;
  message: string;
  ttl: number;
}

function makeId() {
  return (
    globalThis.crypto?.randomUUID?.() ??
    `t-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
  );
}

export const toasts = writable<Toast[]>([]);

function push(tone: ToastTone, message: string, title?: string, ttl = 4200) {
  const id = makeId();
  toasts.update((list) => [...list, { id, tone, title, message, ttl }]);
  if (ttl > 0) setTimeout(() => dismiss(id), ttl);
  return id;
}

export function dismiss(id: string) {
  toasts.update((list) => list.filter((t) => t.id !== id));
}

export const toast = {
  success: (msg: string, title?: string, ttl?: number) =>
    push('success', msg, title, ttl),
  error: (msg: string, title?: string, ttl?: number) =>
    push('error', msg, title, ttl),
  info: (msg: string, title?: string, ttl?: number) =>
    push('info', msg, title, ttl),
  warning: (msg: string, title?: string, ttl?: number) =>
    push('warning', msg, title, ttl),
};
