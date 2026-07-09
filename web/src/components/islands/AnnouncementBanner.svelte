<script lang="ts">
  // Banner de anúncios da instância, exibido no topo de TODAS as páginas do
  // BaseLayout. Auto-carrega em background (client:idle). Se o cidadão
  // estiver logado, chama o endpoint /announcements/active com cookie e o
  // backend já filtra os dismissed. Sem sessão: reusa localStorage pra
  // simular dismissal ("last_seen_ids").
  import { onMount } from 'svelte';
  import {
    listActiveAnnouncements,
    dismissAnnouncement,
    type AnnouncementDto,
  } from '../../lib/api';

  let items = $state<AnnouncementDto[]>([]);
  let loggedIn = $state(false);
  let dismissed = $state<Set<string>>(new Set());

  onMount(async () => {
    try {
      loggedIn = Boolean(localStorage.getItem('dsoc_citizen'));
      dismissed = new Set(
        JSON.parse(localStorage.getItem('dsoc_ann_dismissed') || '[]'),
      );
    } catch { /* storage blocked */ }
    const res = await listActiveAnnouncements();
    if (res.success && res.data) {
      items = res.data;
    }
  });

  function visible(a: AnnouncementDto): boolean {
    return !dismissed.has(a.id);
  }

  async function onDismiss(a: AnnouncementDto) {
    const next = new Set(dismissed).add(a.id);
    dismissed = next;
    try {
      localStorage.setItem('dsoc_ann_dismissed', JSON.stringify([...next]));
    } catch { /* ignore */ }
    if (loggedIn) {
      void dismissAnnouncement(a.id);
    }
  }

  function severityClass(s: string): string {
    return `sev-${s}`;
  }
</script>

{#each items.filter(visible) as a (a.id)}
  <div class="ann" class:sev-critical={a.severity === 'critical'} class:sev-warning={a.severity === 'warning'} role="status">
    <div class="content">{a.body}</div>
    <button
      type="button"
      class="close"
      onclick={() => onDismiss(a)}
      aria-label="Fechar anúncio"
    >
      ✕
    </button>
  </div>
{/each}

<style>
  .ann {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    padding: 0.6rem 1rem;
    background: #dbeafe;
    color: #1e3a8a;
    border-bottom: 1px solid #93c5fd;
    font-size: 0.9rem;
  }
  .sev-warning {
    background: #fef3c7;
    color: #92400e;
    border-bottom-color: #fbbf24;
  }
  .sev-critical {
    background: #fee2e2;
    color: #991b1b;
    border-bottom-color: #f87171;
  }
  .content {
    flex: 1;
    line-height: 1.4;
    white-space: pre-wrap;
  }
  .close {
    background: transparent;
    border: 0;
    color: inherit;
    font: inherit;
    font-size: 1rem;
    padding: 0.25rem 0.5rem;
    border-radius: 4px;
    cursor: pointer;
    opacity: 0.7;
  }
  .close:hover {
    opacity: 1;
    background: rgba(0, 0, 0, 0.06);
  }
</style>
