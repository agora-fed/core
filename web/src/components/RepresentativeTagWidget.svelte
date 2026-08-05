<script lang="ts">
  // Tag-a-representative widget (issue #3): on a forum topic, a citizen marks
  // the mandate who should represent them on this cause. Shows the public
  // aggregate (never individual citizens — LGPD) and the caller's own pick.
  // The tagged mandate receives ONE consolidated e-mail per day (worker).
  import { onMount } from 'svelte';
  import {
    getTopicRepresentatives,
    tagTopicRepresentative,
    untagTopicRepresentative,
    getMandates,
    type TopicRepresentativesDto,
    type MandateDto,
  } from '../lib/api';

  let { topicId }: { topicId: string } = $props();

  let data = $state<TopicRepresentativesDto | null>(null);
  let picking = $state(false);
  let busy = $state(false);
  let notice = $state<string | null>(null);
  let query = $state('');
  let pool = $state<MandateDto[] | null>(null);

  const filtered = $derived(
    (pool ?? [])
      .filter((m) =>
        query.trim().length < 2
          ? false
          : m.display_name.toLowerCase().includes(query.trim().toLowerCase()),
      )
      .slice(0, 8),
  );

  async function reload() {
    const res = await getTopicRepresentatives(topicId);
    if (res.ok && res.data) data = res.data;
  }

  async function openPicker() {
    picking = !picking;
    notice = null;
    if (picking && !pool) {
      // Federal chamber + senate fit in one page each; client-side filter.
      const res = await getMandates(undefined, 100, 0, 'federal');
      pool = res.ok && res.data ? res.data : [];
    }
  }

  async function pick(m: MandateDto) {
    if (busy) return;
    busy = true;
    notice = null;
    const res = await tagTopicRepresentative(topicId, m.id);
    busy = false;
    if (res.success) {
      picking = false;
      query = '';
      await reload();
    } else if (res.error?.code === 'unauthorized') {
      notice = 'Entre na plataforma para marcar quem te representa.';
    } else {
      notice = res.error?.message ?? 'Não foi possível marcar agora.';
    }
  }

  async function untag() {
    if (busy) return;
    busy = true;
    const res = await untagTopicRepresentative(topicId);
    busy = false;
    if (res.success) await reload();
  }

  onMount(reload);
</script>

<section class="rep-widget" aria-label="Quem deve te representar nesta causa">
  <h2 class="rep-title">🗳 Quem deve te representar nesta causa?</h2>
  <p class="muted small">
    Marque um mandato: no fim do dia a plataforma envia a ele um resumo
    consolidado do que a população está cobrando. Números sempre agregados.
  </p>

  {#if data && data.representatives.length > 0}
    <ol class="rep-list">
      {#each data.representatives as r (r.mandate_id)}
        <li class="rep-item" class:mine={data.mine === r.mandate_id}>
          {#if r.avatar_url}
            <img class="rep-avatar" src={r.avatar_url} alt="" loading="lazy" />
          {:else}
            <span class="rep-avatar rep-avatar-ph">👤</span>
          {/if}
          <span class="rep-meta">
            <strong>{r.display_name}</strong>
            <span class="muted small">
              {r.office}{r.party ? ` · ${r.party}` : ''}{r.state ? `-${r.state}` : ''}
            </span>
          </span>
          <span class="rep-count" title="cidadãos que marcaram">
            {r.tag_count.toLocaleString('pt-BR')}
          </span>
          {#if data.mine === r.mandate_id}
            <button type="button" class="rep-untag" onclick={untag} disabled={busy}>
              desmarcar
            </button>
          {/if}
        </li>
      {/each}
    </ol>
  {:else if data}
    <p class="muted small">Ninguém marcou um representante ainda — seja a primeira pessoa.</p>
  {/if}

  <button type="button" class="rep-cta" onclick={openPicker}>
    {picking ? 'Fechar' : data?.mine ? 'Trocar representante' : 'Marcar representante'}
  </button>

  {#if picking}
    <div class="rep-picker">
      <input
        type="search"
        placeholder="Nome do deputado(a) ou senador(a)…"
        bind:value={query}
        aria-label="Buscar representante"
      />
      {#if pool === null}
        <p class="muted small">Carregando mandatos…</p>
      {:else if query.trim().length >= 2 && filtered.length === 0}
        <p class="muted small">Nenhum mandato federal com esse nome.</p>
      {:else}
        <ul class="rep-options">
          {#each filtered as m (m.id)}
            <li>
              <button type="button" onclick={() => pick(m)} disabled={busy}>
                <strong>{m.display_name}</strong>
                <span class="muted small">{m.office}{m.party ? ` · ${m.party}` : ''}</span>
              </button>
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/if}

  {#if notice}
    <p class="rep-notice" role="alert">{notice}</p>
  {/if}
</section>

<style>
  .rep-widget {
    margin: var(--sp-5) 0;
    padding: var(--sp-4);
    border: 1px solid var(--border-subtle);
    border-radius: 10px;
    background: var(--surface-2);
  }
  .rep-title {
    margin: 0 0 var(--sp-1);
    font-size: var(--fs-lg);
  }
  .rep-list {
    list-style: none;
    margin: var(--sp-3) 0;
    padding: 0;
    display: grid;
    gap: var(--sp-2);
  }
  .rep-item {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    padding: var(--sp-2) var(--sp-3);
    border-radius: 8px;
    background: var(--surface-1);
  }
  .rep-item.mine {
    outline: 2px solid var(--accent);
  }
  .rep-avatar {
    width: 36px;
    height: 36px;
    border-radius: 50%;
    object-fit: cover;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: var(--surface-3);
    flex: none;
  }
  .rep-meta {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .rep-count {
    margin-left: auto;
    font-weight: 700;
    color: var(--accent-strong);
  }
  .rep-untag {
    background: none;
    border: none;
    color: var(--text-3);
    cursor: pointer;
    font-size: var(--fs-xs);
    text-decoration: underline;
  }
  .rep-cta {
    margin-top: var(--sp-2);
  }
  .rep-picker {
    margin-top: var(--sp-3);
  }
  .rep-picker input {
    width: 100%;
  }
  .rep-options {
    list-style: none;
    margin: var(--sp-2) 0 0;
    padding: 0;
    display: grid;
    gap: var(--sp-1);
  }
  .rep-options button {
    width: 100%;
    text-align: left;
    display: flex;
    flex-direction: column;
    padding: var(--sp-2) var(--sp-3);
    border-radius: 8px;
    border: 1px solid var(--border-subtle);
    background: var(--surface-1);
    cursor: pointer;
  }
  .rep-options button:hover {
    border-color: var(--accent);
  }
  .rep-notice {
    margin-top: var(--sp-2);
    color: var(--warning);
  }
  .muted {
    color: var(--text-3);
  }
  .small {
    font-size: var(--fs-sm);
  }
</style>
