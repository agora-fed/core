<script lang="ts">
  // Página pública do grupo de campanha. /grupo/?id=<uuid>
  // O eleitor vê a descrição + atualizações da campanha e entra/sai (join/leave).
  import { onMount } from 'svelte';
  import {
    getCampaignGroup,
    joinCampaignGroup,
    leaveCampaignGroup,
    type PublicCampaignGroup,
  } from '../../lib/api';

  let loading = $state(true);
  let error = $state<string | null>(null);
  let group = $state<PublicCampaignGroup | null>(null);
  let busy = $state(false);
  let loggedIn = $state(false);

  const fmtDate = new Intl.DateTimeFormat('pt-BR', { day: '2-digit', month: 'short', year: 'numeric' });

  async function load(id: string) {
    const res = await getCampaignGroup(id);
    if (res.ok && res.data) group = res.data;
    else error = res.error ?? 'Grupo não encontrado.';
  }

  async function toggleMembership() {
    if (!group || busy) return;
    if (!loggedIn) {
      window.location.href = `/entrar/?next=${encodeURIComponent(window.location.pathname + window.location.search)}`;
      return;
    }
    busy = true;
    const res = group.sou_membro
      ? await leaveCampaignGroup(group.id)
      : await joinCampaignGroup(group.id);
    busy = false;
    if (res.ok) await load(group.id);
  }

  onMount(async () => {
    try {
      loggedIn = Boolean(localStorage.getItem('dsoc_citizen'));
    } catch {
      loggedIn = false;
    }
    const id = new URLSearchParams(window.location.search).get('id') ?? '';
    if (!id) {
      error = 'Grupo não informado.';
      loading = false;
      return;
    }
    await load(id);
    loading = false;
  });
</script>

{#if loading}
  <p class="muted">Carregando grupo…</p>
{:else if error || !group}
  <div class="card center">
    <p class="hint-error" role="alert">{error ?? 'Grupo não encontrado.'}</p>
    <p class="muted"><a href="/politicos">Ver políticos</a></p>
  </div>
{:else}
  <header class="head">
    <h1>{group.name}</h1>
    <p class="owner muted">
      Grupo de campanha de
      {#if group.owner_handle}
        <a href={`/perfil/?u=${encodeURIComponent(group.owner_handle)}`}>{group.owner_display_name ?? '@' + group.owner_handle}</a>
      {:else}
        <a href={`/politicos/${group.mandate_id}`}>{group.owner_display_name ?? 'político(a)'}</a>
      {/if}
    </p>
    {#if group.description}
      <p class="desc">{group.description}</p>
    {/if}
    <div class="cta-row">
      <button
        class="btn"
        class:joined={group.sou_membro}
        onclick={toggleMembership}
        disabled={busy}
      >
        {busy ? '…' : group.sou_membro ? '✓ Você participa — sair' : 'Participar da campanha'}
      </button>
      <span class="members muted">{group.member_count} apoiador{group.member_count === 1 ? '' : 'es'}</span>
    </div>
  </header>

  <section class="updates">
    <h2>Atualizações</h2>
    {#if group.posts.length === 0}
      <p class="muted">A campanha ainda não publicou nada.</p>
    {:else}
      <ul class="posts">
        {#each group.posts as p (p.id)}
          <li class="card post">
            <p class="post-body">{p.body}</p>
            <time class="muted">{fmtDate.format(new Date(p.created_at))}</time>
          </li>
        {/each}
      </ul>
    {/if}
  </section>
{/if}

<style>
  .head { margin-bottom: 2rem; }
  .head h1 { margin: 0 0 0.3rem; font-size: 1.8rem; }
  .owner { margin: 0 0 0.75rem; }
  .owner a { color: var(--accent-strong, #15803d); text-decoration: none; }
  .owner a:hover { text-decoration: underline; }
  .desc { margin: 0 0 1.2rem; font-size: 1.05rem; color: var(--text-2, inherit); }
  .cta-row { display: flex; align-items: center; gap: 1rem; flex-wrap: wrap; }
  .btn {
    padding: 0.6rem 1.3rem;
    border-radius: 10px;
    border: none;
    background: var(--accent, #15803d);
    color: var(--accent-contrast, #fff);
    font-weight: 700;
    font-size: 1rem;
    cursor: pointer;
    transition: filter 100ms ease;
  }
  .btn:hover:not(:disabled) { filter: brightness(1.05); }
  .btn:disabled { opacity: 0.6; cursor: default; }
  .btn.joined {
    background: var(--surface-2, #eef);
    color: var(--text-1, inherit);
    border: 1px solid var(--border-subtle, #ccc);
  }
  .members { font-variant-numeric: tabular-nums; font-weight: 600; }

  .updates h2 { font-size: 1.2rem; margin: 0 0 1rem; }
  .posts { list-style: none; padding: 0; margin: 0; display: grid; gap: 0.8rem; }
  .post { padding: 1rem 1.2rem; }
  .post-body { margin: 0 0 0.5rem; white-space: pre-wrap; }
  .post time { font-size: 0.8rem; }
  .card {
    background: var(--surface-1, #fff);
    border: 1px solid var(--border-subtle, rgba(0,0,0,0.1));
    border-radius: 12px;
  }
  .center { text-align: center; padding: 2.5rem 1.5rem; }
  .hint-error { color: #dc2626; }
</style>
