<script lang="ts">
  // Contribuições de outros servidores: fila de aprovação e bloqueios.
  import { onMount } from 'svelte';
  import {
    addForumInboundBlock,
    decideForumInbound,
    listForumInboundBlocks,
    listForumInboundQueue,
    removeForumInboundBlock,
    type ForumInboundBlock,
    type ForumInboundItem,
  } from '../../lib/api';

  let queue = $state<ForumInboundItem[]>([]);
  let blocks = $state<ForumInboundBlock[]>([]);
  let message = $state<string | null>(null);
  let busy = $state(false);
  let kind = $state<'actor' | 'domain'>('domain');
  let value = $state('');

  async function load() {
    const [items, blocked] = await Promise.all([listForumInboundQueue(), listForumInboundBlocks()]);
    queue = items.data ?? [];
    blocks = blocked.data ?? [];
    if (!items.success || !blocked.success) message = 'Não foi possível carregar a fila.';
  }

  async function run(action: () => Promise<{ success: boolean; error?: { message?: string } | null }>) {
    busy = true;
    message = null;
    const res = await action();
    if (!res.success) message = res.error?.message ?? 'Não foi possível concluir.';
    await load();
    busy = false;
    return res.success;
  }

  const decide = (item: ForumInboundItem, decision: 'approve' | 'reject') =>
    run(() => decideForumInbound(item.kind, item.id, decision));

  const blockAuthor = (item: ForumInboundItem) =>
    run(() => addForumInboundBlock({ kind: 'actor', value: item.actor_url }));

  async function addBlock(event: SubmitEvent) {
    event.preventDefault();
    if (await run(() => addForumInboundBlock({ kind, value }))) value = '';
  }

  onMount(load);
</script>

<section>
  <h2>Contribuições aguardando aprovação</h2>
  {#if message}<p role="alert">{message}</p>{/if}
  {#if queue.length === 0}
    <p class="muted">Nada na fila.</p>
  {/if}
  <ul class="list">
    {#each queue as item (item.id)}
      <li>
        <p class="muted">
          {item.kind === 'topic' ? 'Tópico' : 'Comentário'} em /f/{item.forum_path} ·
          {item.handle ?? item.actor_url}
        </p>
        {#if item.title}<strong>{item.title}</strong>{/if}
        <p class="body">{item.body}</p>
        <button disabled={busy} onclick={() => decide(item, 'approve')}>Aprovar</button>
        <button disabled={busy} onclick={() => decide(item, 'reject')}>Recusar</button>
        <button disabled={busy} onclick={() => blockAuthor(item)}>Bloquear autor</button>
      </li>
    {/each}
  </ul>

  <h2>Bloqueios</h2>
  <form onsubmit={addBlock}>
    <label>
      Tipo
      <select bind:value={kind}>
        <option value="domain">Servidor (domínio)</option>
        <option value="actor">Conta (URL)</option>
      </select>
    </label>
    <label>
      Valor
      <input bind:value required maxlength="500" />
    </label>
    <button type="submit" disabled={busy}>Bloquear</button>
  </form>
  <ul class="list">
    {#each blocks as block (block.id)}
      <li>
        {block.value}
        <button disabled={busy} onclick={() => run(() => removeForumInboundBlock(block.kind, block.id))}>
          Remover
        </button>
      </li>
    {/each}
  </ul>
</section>

<style>
  section {
    margin-top: var(--sp-6);
  }
  .list {
    list-style: none;
    padding: 0;
  }
  .list li {
    border-bottom: 1px solid var(--border-subtle);
    padding: var(--sp-3) 0;
    overflow-wrap: anywhere;
  }
  .body {
    white-space: pre-wrap;
  }
  form {
    display: flex;
    flex-wrap: wrap;
    gap: var(--sp-3);
    align-items: end;
  }
</style>
