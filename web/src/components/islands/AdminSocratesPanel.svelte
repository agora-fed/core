<script lang="ts">
  // Painel admin do SOCRATES (migration 0670): cola a URL/ID de uma Ideia
  // Legislativa do e-Cidadania (Senado) → o gateway busca o título e cria um
  // tópico no fórum `senado` assinado pelo bot, abrindo o debate completo
  // (favor × contra) que o portal do Senado não permite. Dedup por ideia:
  // 409 = já espelhada (o backend devolve o tópico existente em `data`).
  import { onMount } from 'svelte';
  import {
    getSocratesMirrors,
    socratesMirrorIdea,
    type SocratesMirrorEntry,
    type SocratesMirrorCreated,
  } from '../../lib/api';

  let items = $state<SocratesMirrorEntry[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);

  let input = $state('');
  let mirroring = $state(false);
  /** Último resultado (criado OU já existente) pra mostrar o link do tópico. */
  let result = $state<{ kind: 'created' | 'duplicate'; path: string } | null>(null);

  async function load() {
    loading = true;
    const res = await getSocratesMirrors();
    loading = false;
    if (res.success && res.data) {
      items = res.data;
    } else {
      error = res.error?.message ?? 'Não foi possível carregar os espelhos.';
    }
  }

  async function mirror() {
    if (!input.trim()) {
      error = 'Cole a URL da ideia no e-Cidadania ou o id numérico.';
      return;
    }
    mirroring = true;
    error = null;
    result = null;
    const res = await socratesMirrorIdea(input.trim());
    mirroring = false;
    if (res.success && res.data) {
      result = { kind: 'created', path: res.data.path };
      input = '';
      await load();
      return;
    }
    if (res.error?.code === 'already_mirrored') {
      const dup = res.data as SocratesMirrorCreated | null;
      if (dup?.path) {
        result = { kind: 'duplicate', path: dup.path };
      }
      error = res.error.message ?? 'Esta ideia já foi espelhada.';
      return;
    }
    error = res.error?.message ?? 'Não foi possível espelhar a ideia.';
  }

  function fmtDate(iso: string): string {
    try {
      return new Date(iso).toLocaleDateString('pt-BR', {
        day: '2-digit',
        month: '2-digit',
        year: 'numeric',
        hour: '2-digit',
        minute: '2-digit',
      });
    } catch {
      return iso;
    }
  }

  onMount(load);
</script>

<div class="create card">
  <h2>Espelhar Ideia Legislativa</h2>
  <p class="muted small">
    Cole a URL da ideia (ex.:
    <code>https://www12.senado.leg.br/ecidadania/visualizacaoideia?id=165188</code>) ou só o id
    numérico. O SOCRATES cria o tópico no fórum <strong>Senado Federal</strong> com o título da
    ideia e o link original — cada ideia é espelhada no máximo uma vez.
  </p>
  <div class="fields">
    <label class="grow">
      <span>URL ou id da ideia</span>
      <input
        type="text"
        bind:value={input}
        placeholder="https://www12.senado.leg.br/ecidadania/visualizacaoideia?id=…"
        onkeydown={(e) => e.key === 'Enter' && mirror()}
      />
    </label>
    <button class="btn" onclick={mirror} disabled={mirroring}>
      {mirroring ? 'Espelhando…' : 'Espelhar ideia'}
    </button>
  </div>
</div>

{#if result?.kind === 'created'}
  <div class="card ok" role="status">
    Ideia espelhada! <a href={result.path}>Abrir o tópico criado →</a>
  </div>
{/if}
{#if error}
  <div class="card err" role="alert">
    {error}
    {#if result?.kind === 'duplicate'}
      <a href={result.path}>Ver o tópico existente →</a>
    {/if}
  </div>
{/if}

<div class="table-wrap">
  <table>
    <thead>
      <tr><th>Ideia</th><th>Tópico</th><th>Espelhada em</th><th>Original</th></tr>
    </thead>
    <tbody>
      {#if loading}
        <tr><td colspan="4" class="muted center">Carregando…</td></tr>
      {:else if items.length === 0}
        <tr><td colspan="4" class="muted center">Nenhuma ideia espelhada ainda.</td></tr>
      {:else}
        {#each items as m (m.ideia_id)}
          <tr>
            <td class="mono small">#{m.ideia_id}</td>
            <td><a href={m.path}>{m.topic_title}</a></td>
            <td class="small">{fmtDate(m.created_at)}</td>
            <td class="small">
              <a href={m.source_url} target="_blank" rel="noopener noreferrer">e-Cidadania ↗</a>
            </td>
          </tr>
        {/each}
      {/if}
    </tbody>
  </table>
</div>

<style>
  .card { background: var(--surface-1, #fff); border: 1px solid var(--border-subtle, rgba(0,0,0,0.1)); border-radius: 12px; padding: 1rem 1.2rem; margin-bottom: 1rem; }
  .ok { color: #15803d; border-color: #15803d; }
  .err { color: #dc2626; border-color: #dc2626; }
  .err a, .ok a { color: inherit; font-weight: 600; }
  .create h2 { margin: 0 0 0.4rem; font-size: 1rem; }
  .create p { margin: 0 0 0.8rem; }
  code { font-size: 0.8em; word-break: break-all; }
  .fields { display: flex; gap: 0.6rem; flex-wrap: wrap; align-items: end; }
  label { display: grid; gap: 0.2rem; font-size: 0.8rem; color: var(--muted, #64748b); }
  label.grow { flex: 1 1 22rem; }
  input { padding: 0.5rem 0.6rem; border-radius: 8px; border: 1px solid var(--border-subtle, #cbd5e1); background: var(--surface-1, #fff); color: inherit; font: inherit; width: 100%; }
  .btn { padding: 0.5rem 1rem; border-radius: 8px; border: 1px solid var(--c-ink, #0f172a); background: var(--surface-1, #fff); color: inherit; font-weight: 600; cursor: pointer; }
  .btn:disabled { opacity: 0.5; cursor: not-allowed; }
  .table-wrap { overflow-x: auto; border: 1px solid var(--border-subtle, rgba(0,0,0,0.1)); border-radius: 12px; }
  table { width: 100%; border-collapse: collapse; font-size: 0.92rem; }
  th, td { text-align: left; padding: 0.55rem 0.8rem; border-bottom: 1px solid var(--border-subtle, rgba(0,0,0,0.06)); vertical-align: middle; }
  th { font-size: 0.75rem; text-transform: uppercase; letter-spacing: 0.03em; color: var(--muted, #64748b); }
  .small { font-size: 0.82rem; }
  .center { text-align: center; }
  .mono { font-family: ui-monospace, monospace; }
  .muted { color: var(--muted, #64748b); }
</style>
