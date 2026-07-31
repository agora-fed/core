<script lang="ts">
  // Painel admin do SOCRATES (migrations 0670/0671). Dois caminhos pro mesmo
  // espelho: (a) colar a URL/ID de uma Ideia Legislativa do e-Cidadania, e
  // (b) o sweep automático, que descobre as ideias em alta no portal do Senado.
  // Nos dois casos o gateway cria um tópico no fórum `senado` assinado pelo bot,
  // abrindo o debate completo (favor × contra) que o portal não permite.
  // Dedup por ideia: 409 = já espelhada (o backend devolve o tópico em `data`).
  import { onMount } from 'svelte';
  import {
    getSocratesMirrors,
    getSocratesSweepRuns,
    runSocratesSweep,
    socratesBackfill,
    socratesMirrorIdea,
    type SocratesBackfillStats,
    type SocratesMirrorEntry,
    type SocratesMirrorCreated,
    type SocratesSweepRun,
    type SocratesSweepStats,
  } from '../../lib/api';

  let items = $state<SocratesMirrorEntry[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);

  let input = $state('');
  let mirroring = $state(false);
  /** Último resultado (criado OU já existente) pra mostrar o link do tópico. */
  let result = $state<{ kind: 'created' | 'duplicate'; path: string } | null>(null);

  let runs = $state<SocratesSweepRun[]>([]);
  let sweeping = $state(false);
  /** Resultado da última rodada disparada nesta sessão do painel. */
  let sweepResult = $state<SocratesSweepStats | null>(null);
  let sweepError = $state<string | null>(null);

  let backfilling = $state(false);
  let backfillResult = $state<SocratesBackfillStats | null>(null);
  let backfillError = $state<string | null>(null);

  /** Espelhos ainda no formato pré-v3 (corpo só com o título): o número que
   *  justifica o botão de backfill. */
  const pendentes = $derived(items.filter((m) => !m.body_synced_at).length);

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

  async function loadRuns() {
    const res = await getSocratesSweepRuns();
    if (res.success && res.data) {
      runs = res.data;
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

  async function sweep() {
    sweeping = true;
    sweepError = null;
    sweepResult = null;
    const res = await runSocratesSweep();
    sweeping = false;
    if (res.success && res.data) {
      sweepResult = res.data;
      // A rodada pode ter criado tópicos e atualizado apoios: recarrega as duas.
      await Promise.all([load(), loadRuns()]);
      return;
    }
    sweepError = res.error?.message ?? 'Não foi possível rodar o sweep.';
    await loadRuns();
  }

  async function backfill() {
    backfilling = true;
    backfillError = null;
    backfillResult = null;
    const res = await socratesBackfill();
    backfilling = false;
    if (res.success && res.data) {
      backfillResult = res.data;
      await load();
      return;
    }
    backfillError = res.error?.message ?? 'Não foi possível preencher os espelhos.';
  }

  /** Apoios como o painel mostra: o número da v3 formatado em pt-BR; na falta
   *  dele, o texto que o Senado já formatou. */
  function fmtApoios(m: SocratesMirrorEntry): string | null {
    if (m.apoiamentos_num !== null) return m.apoiamentos_num.toLocaleString('pt-BR');
    return m.apoiamentos;
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

  onMount(() => {
    void load();
    void loadRuns();
  });
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

<div class="create card">
  <h2>Sweep automático</h2>
  <p class="muted small">
    O sweep lê as Ideias Legislativas <strong>em alta</strong> no e-Cidadania, espelha as que ainda
    não têm tópico e atualiza o número de apoios das já espelhadas. No servidor ele roda a cada 6
    horas quando ligado (<code>SOCRATES_SWEEP_ENABLED=true</code>); aqui você dispara uma rodada na
    hora.
  </p>
  <div class="fields">
    <button class="btn" onclick={sweep} disabled={sweeping}>
      {sweeping ? 'Rodando…' : 'Rodar sweep agora'}
    </button>
  </div>
</div>

{#if sweepResult}
  <div class="card ok" role="status">
    Rodada concluída: <strong>{sweepResult.found}</strong> ideias encontradas,
    <strong>{sweepResult.mirrored}</strong> espelhadas, <strong>{sweepResult.skipped}</strong>
    puladas, <strong>{sweepResult.updated}</strong> com apoios atualizados,
    <strong>{sweepResult.refreshed}</strong> com o texto do tópico reescrito.
    {#if sweepResult.errors.length > 0}
      <ul class="small">
        {#each sweepResult.errors as e (e)}
          <li>{e}</li>
        {/each}
      </ul>
    {/if}
  </div>
{/if}
{#if sweepError}
  <div class="card err" role="alert">{sweepError}</div>
{/if}

<div class="create card">
  <h2>Preencher espelhos antigos</h2>
  <p class="muted small">
    Os espelhos criados antes desta versão têm no tópico <strong>só o título</strong> da ideia — sem
    a proposta, sem o número de apoios e sem a situação no Senado. Este botão busca cada ideia
    espelhada no e-Cidadania e <strong>reescreve o texto do tópico</strong> com a pauta completa, os
    apoios e a situação. Só o texto de abertura muda: votos, comentários e pontuação do debate
    continuam intactos. Pode rodar quantas vezes quiser — o que já está em dia não é reescrito.
    {#if pendentes > 0}
      <br /><strong>{pendentes}</strong>
      {pendentes === 1 ? 'espelho ainda está' : 'espelhos ainda estão'} no formato antigo.
    {/if}
  </p>
  <div class="fields">
    <button class="btn" onclick={backfill} disabled={backfilling}>
      {backfilling ? 'Preenchendo…' : 'Preencher espelhos antigos'}
    </button>
  </div>
</div>

{#if backfillResult}
  <div class="card ok" role="status">
    Backfill concluído: <strong>{backfillResult.total}</strong>
    {backfillResult.total === 1 ? 'espelho verificado' : 'espelhos verificados'},
    <strong>{backfillResult.refreshed}</strong> atualizados.
    {#if backfillResult.errors.length > 0}
      <ul class="small">
        {#each backfillResult.errors as e (e)}
          <li>{e}</li>
        {/each}
      </ul>
    {/if}
  </div>
{/if}
{#if backfillError}
  <div class="card err" role="alert">{backfillError}</div>
{/if}

<h3>Ideias espelhadas</h3>
<div class="table-wrap">
  <table>
    <thead>
      <tr>
        <th>Ideia</th>
        <th>Tópico</th>
        <th>Apoios no Senado</th>
        <th>Situação</th>
        <th>Origem</th>
        <th>Espelhada em</th>
        <th>Original</th>
      </tr>
    </thead>
    <tbody>
      {#if loading}
        <tr><td colspan="7" class="muted center">Carregando…</td></tr>
      {:else if items.length === 0}
        <tr><td colspan="7" class="muted center">Nenhuma ideia espelhada ainda.</td></tr>
      {:else}
        {#each items as m (m.ideia_id)}
          <tr>
            <td class="mono small">#{m.ideia_id}</td>
            <td>
              <a href={m.path}>{m.topic_title}</a>
              {#if !m.body_synced_at}
                <span class="tag pendente" title="O tópico ainda tem só o título da ideia">
                  sem pauta
                </span>
              {/if}
            </td>
            <td class="small">
              {#if fmtApoios(m)}
                <strong>{fmtApoios(m)}</strong>
                {#if m.apoios_updated_at}
                  <span class="muted"> · lido em {fmtDate(m.apoios_updated_at)}</span>
                {/if}
              {:else}
                <span class="muted">—</span>
              {/if}
            </td>
            <td class="small">
              {#if m.situacao}
                {m.situacao}
              {:else}
                <span class="muted">—</span>
              {/if}
            </td>
            <td class="small">
              <span class="tag" class:sweep={m.origin === 'sweep'}>
                {m.origin === 'sweep' ? 'sweep' : 'manual'}
              </span>
            </td>
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

<h3>Últimas rodadas do sweep</h3>
<div class="table-wrap">
  <table>
    <thead>
      <tr>
        <th>Início</th>
        <th>Fim</th>
        <th>Encontradas</th>
        <th>Espelhadas</th>
        <th>Puladas</th>
        <th>Erro</th>
      </tr>
    </thead>
    <tbody>
      {#if runs.length === 0}
        <tr><td colspan="6" class="muted center">Nenhuma rodada registrada ainda.</td></tr>
      {:else}
        {#each runs as r (r.id)}
          <tr>
            <td class="small">{fmtDate(r.started_at)}</td>
            <td class="small">
              {#if r.finished_at}
                {fmtDate(r.finished_at)}
              {:else}
                <span class="muted">em curso…</span>
              {/if}
            </td>
            <td class="small">{r.found}</td>
            <td class="small">{r.mirrored}</td>
            <td class="small">{r.skipped}</td>
            <td class="small">
              {#if r.error}
                <span class="failed">{r.error}</span>
              {:else}
                <span class="muted">—</span>
              {/if}
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
  .ok ul { margin: 0.5rem 0 0; padding-left: 1.2rem; }
  .create h2 { margin: 0 0 0.4rem; font-size: 1rem; }
  .create p { margin: 0 0 0.8rem; }
  h3 { font-size: 0.95rem; margin: 1.4rem 0 0.5rem; }
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
  .failed { color: #dc2626; }
  .tag { display: inline-block; padding: 0.1rem 0.45rem; border-radius: 999px; border: 1px solid var(--border-subtle, #cbd5e1); font-size: 0.72rem; text-transform: uppercase; letter-spacing: 0.03em; color: var(--muted, #64748b); }
  .tag.sweep { border-color: #15803d; color: #15803d; }
  .tag.pendente { border-color: #b45309; color: #b45309; margin-left: 0.4rem; }
</style>
