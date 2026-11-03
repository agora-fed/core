<script lang="ts">
  import { onMount } from 'svelte';
  import { getConsulta, responderConsulta, votarConsulta } from '../../lib/api';
  import { formatDate } from '../../lib/format';
  import type { ConsultaDetail } from '../../lib/types';

  type Choice = 'concordo' | 'neutro' | 'discordo';
  const PLEBISCITO_OPTIONS: { key: Choice; label: string }[] = [
    { key: 'concordo', label: 'Concordo' },
    { key: 'neutro', label: 'Neutro' },
    { key: 'discordo', label: 'Discordo' },
  ];

  let loading = $state(true);
  let error = $state<string | null>(null);
  let consulta = $state<ConsultaDetail | null>(null);
  let loggedIn = $state(false);
  let busy = $state(false);
  let saved = $state(false);
  let picks = $state<Record<string, Choice>>({});
  let votePick = $state<string | null>(null);

  const isOpen = $derived(consulta?.status === 'open');
  const isPlebiscito = $derived(consulta?.kind === 'plebiscito');
  const kindLabel = $derived(isPlebiscito ? 'Plebiscito' : 'Consulta');
  const statusLabel = $derived.by(() => {
    if (!consulta) return '';
    const fem = !isPlebiscito;
    return isOpen
      ? `${kindLabel} ${fem ? 'aberta' : 'aberto'}`
      : `${kindLabel} ${fem ? 'encerrada' : 'encerrado'}`;
  });

  function pct(n: number, total: number): number {
    return total > 0 ? Math.round((n / total) * 100) : 0;
  }

  function seedPicks(c: ConsultaDetail) {
    if (c.kind === 'plebiscito') {
      const next: Record<string, Choice> = {};
      for (const q of c.questions) if (q.my_answer) next[q.id] = q.my_answer;
      picks = next;
    } else {
      const voted = c.questions.find((q) => q.my_vote);
      votePick = voted?.id ?? null;
    }
  }

  function voteTallyFor(qid: string): number {
    return consulta?.questions.find((q) => q.id === qid)?.vote_tally?.votes ?? 0;
  }

  function totalVotes(): number {
    return consulta?.questions.reduce((s, q) => s + (q.vote_tally?.votes ?? 0), 0) ?? 0;
  }

  async function load(id: string) {
    const res = await getConsulta(id);
    if (res.success && res.data) {
      consulta = res.data;
      seedPicks(res.data);
    } else {
      error = res.error?.message ?? 'Consulta não encontrada.';
    }
  }

  function choose(qid: string, choice: Choice) {
    if (!isOpen) return;
    if (!loggedIn) {
      window.location.href = `/entrar/?next=${encodeURIComponent(window.location.pathname + window.location.search)}`;
      return;
    }
    picks = { ...picks, [qid]: choice };
    saved = false;
  }

  function chooseOption(qid: string) {
    if (!isOpen) return;
    if (!loggedIn) {
      window.location.href = `/entrar/?next=${encodeURIComponent(window.location.pathname + window.location.search)}`;
      return;
    }
    votePick = qid;
    saved = false;
  }

  async function submitPlebiscito() {
    if (!consulta || busy) return;
    const answers = Object.entries(picks).map(([question_id, answer]) => ({ question_id, answer }));
    if (answers.length === 0) return;
    busy = true;
    error = null;
    const res = await responderConsulta(consulta.id, answers);
    busy = false;
    if (res.success) {
      saved = true;
      await load(consulta.id);
    } else {
      error = res.error?.message ?? 'Não foi possível enviar suas respostas.';
    }
  }

  async function submitVote() {
    if (!consulta || busy || !votePick) return;
    busy = true;
    error = null;
    const res = await votarConsulta(consulta.id, votePick);
    busy = false;
    if (res.success) {
      saved = true;
      await load(consulta.id);
    } else {
      error = res.error?.message ?? 'Não foi possível registrar seu voto.';
    }
  }

  const submit = $derived(isPlebiscito ? submitPlebiscito : submitVote);
  const backHref = $derived(isPlebiscito ? '/plebiscitos' : '/consultas');
  const backLabel = $derived(isPlebiscito ? '← Todos os plebiscitos' : '← Todas as consultas');

  onMount(async () => {
    try {
      loggedIn = Boolean(localStorage.getItem('dsoc_citizen'));
    } catch {
      loggedIn = false;
    }
    const id = new URLSearchParams(window.location.search).get('id') ?? '';
    if (!id) {
      error = 'Consulta não informada.';
      loading = false;
      return;
    }
    await load(id);
    loading = false;
  });
</script>

{#if loading}
  <p class="muted">Carregando consulta…</p>
{:else if error && !consulta}
  <div class="card center">
    <p class="hint-error" role="alert">{error}</p>
    <p class="muted"><a href="/consultas">Ver todas as consultas</a></p>
  </div>
{:else if consulta}
  <header class="head">
    <span class="badge {consulta.status}">{statusLabel}</span>
    {#if isPlebiscito}<span class="badge sso">Somente membros</span>{/if}
    <h1>{consulta.title}</h1>
    <p class="muted">
      {isOpen ? 'Encerra em' : 'Encerrou em'} {formatDate(consulta.closes_at)}
    </p>
  </header>

  {#if !loggedIn && isOpen}
    <div class="card notice">
      {#if isPlebiscito}
        <p><strong>Plebiscito interno.</strong> Somente membros autenticados da Orgânica podem votar.</p>
      {:else}
        <p><strong>Faça parte.</strong> Entre para registrar seu voto — os resultados abaixo são públicos.</p>
      {/if}
      <a class="btn" href={`/entrar/?next=${encodeURIComponent('/consulta/?id=' + consulta.id)}`}>Entrar para responder</a>
    </div>
  {/if}

  {#if isPlebiscito}
    <!-- Plebiscito: concordo/neutro/discordo per question -->
    <ol class="questions">
      {#each consulta.questions as q (q.id)}
        <li class="card q">
          <p class="prompt">{q.prompt}</p>

          {#if isOpen}
            <div class="options" role="group" aria-label="Sua resposta">
              {#each PLEBISCITO_OPTIONS as opt (opt.key)}
                <button
                  type="button"
                  class="opt {opt.key}"
                  class:selected={picks[q.id] === opt.key}
                  aria-pressed={picks[q.id] === opt.key}
                  onclick={() => choose(q.id, opt.key)}
                >{opt.label}</button>
              {/each}
            </div>
          {/if}

          {#if q.tally}
          <div class="results" aria-label="Resultado parcial">
            {#each PLEBISCITO_OPTIONS as opt (opt.key)}
              {@const n = q.tally[opt.key]}
              <div class="bar-row">
                <span class="bar-label">{opt.label}</span>
                <div class="bar-track">
                  <div class="bar-fill {opt.key}" style={`width:${pct(n, q.tally.total)}%`}></div>
                </div>
                <span class="bar-val muted">{pct(n, q.tally.total)}% · {n}</span>
              </div>
            {/each}
            <p class="total muted small">{q.tally.total} {q.tally.total === 1 ? 'resposta' : 'respostas'}</p>
          </div>
          {/if}
        </li>
      {/each}
    </ol>
  {:else}
    <!-- Consulta: single-choice (pick one option) -->
    {@const total = totalVotes()}
    <ol class="questions">
      {#each consulta.questions as q (q.id)}
        {@const votes = voteTallyFor(q.id)}
        <li class="card q">
          {#if isOpen}
            <button
              type="button"
              class="opt-choice"
              class:selected={votePick === q.id}
              aria-pressed={votePick === q.id}
              onclick={() => chooseOption(q.id)}
            >
              <span class="radio" class:checked={votePick === q.id}></span>
              <span class="opt-text">{q.prompt}</span>
            </button>
          {:else}
            <p class="prompt">{q.prompt}</p>
          {/if}

          <div class="bar-row">
            <span class="bar-label muted">{votes} {votes === 1 ? 'voto' : 'votos'}</span>
            <div class="bar-track">
              <div class="bar-fill vote-bar" style={`width:${pct(votes, total)}%`}></div>
            </div>
            <span class="bar-val muted">{pct(votes, total)}%</span>
          </div>
        </li>
      {/each}
    </ol>
    <p class="total muted small" style="margin-top:0.5rem">{total} {total === 1 ? 'voto' : 'votos'} no total</p>
  {/if}

  {#if isOpen && loggedIn}
    <div class="submit-bar">
      {#if error}<p class="hint-error" role="alert">{error}</p>{/if}
      {#if saved}<p class="hint-ok">{isPlebiscito ? 'Respostas registradas.' : 'Voto registrado.'} Obrigado por participar!</p>{/if}
      <button
        class="btn primary"
        onclick={submit}
        disabled={busy || (isPlebiscito ? Object.keys(picks).length === 0 : !votePick)}
      >
        {busy ? 'Enviando…' : isPlebiscito ? 'Enviar respostas' : 'Votar'}
      </button>
    </div>
  {/if}

  <p class="muted back"><a href={backHref}>{backLabel}</a></p>
{/if}

<style>
  .head { margin-bottom: 1.5rem; }
  .head h1 { margin: 0.5rem 0 0.35rem; }
  .badge { font-size: 0.72rem; font-weight: 700; text-transform: uppercase; letter-spacing: 0.03em; padding: 0.2rem 0.6rem; border-radius: 999px; }
  .badge.open { background: var(--c-green-soft, #dcfce7); color: var(--c-green-dark, #15803d); }
  .badge.closed { background: var(--c-bg, #f1f5f9); color: var(--muted, #64748b); }
  .badge.sso { background: var(--c-blue-soft, #dbeafe); color: var(--c-blue-dark, #1d4ed8); margin-left: 0.35rem; }
  .card { background: var(--surface-1, var(--c-paper)); border: 1px solid var(--border-subtle, var(--c-border)); border-radius: 12px; padding: 1.25rem; }
  .center { text-align: center; padding: 2.5rem 1.5rem; }
  .notice { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: 1rem; margin-bottom: 1.5rem; background: var(--c-green-soft, #f0fdf4); }
  .questions { list-style: none; padding: 0; margin: 0; display: grid; gap: 1rem; }
  .q { display: grid; gap: 1rem; }
  .prompt { font-size: 1.1rem; font-weight: 600; margin: 0; }
  .options { display: flex; gap: 0.5rem; flex-wrap: wrap; }
  .opt { flex: 1 1 auto; min-width: 6rem; padding: 0.6rem 0.75rem; border-radius: 8px; border: 1.5px solid var(--c-border, #cbd5e1); background: var(--c-paper, #fff); font-weight: 600; cursor: pointer; transition: border-color 100ms, background 100ms; }
  .opt:hover { border-color: var(--c-ink, #0f172a); }
  .opt.selected.concordo { border-color: #15803d; background: #dcfce7; color: #14532d; }
  .opt.selected.neutro { border-color: #b45309; background: #fef3c7; color: #78350f; }
  .opt.selected.discordo { border-color: #b91c1c; background: #fee2e2; color: #7f1d1d; }
  .opt-choice { display: flex; align-items: center; gap: 0.75rem; width: 100%; padding: 0.75rem 1rem; border-radius: 8px; border: 1.5px solid var(--c-border, #cbd5e1); background: var(--c-paper, #fff); font: inherit; font-weight: 600; font-size: 1.05rem; cursor: pointer; transition: border-color 100ms, background 100ms; text-align: left; }
  .opt-choice:hover { border-color: var(--c-ink, #0f172a); }
  .opt-choice.selected { border-color: var(--c-green-dark, #15803d); background: var(--c-green-soft, #dcfce7); }
  .radio { width: 1.1rem; height: 1.1rem; border-radius: 50%; border: 2px solid var(--c-border, #cbd5e1); flex-shrink: 0; transition: border-color 100ms, background 100ms; }
  .radio.checked { border-color: var(--c-green-dark, #15803d); background: var(--c-green-dark, #15803d); box-shadow: inset 0 0 0 2.5px var(--c-paper, #fff); }
  .results { display: grid; gap: 0.4rem; }
  .bar-row { display: grid; grid-template-columns: 5rem 1fr auto; align-items: center; gap: 0.6rem; font-size: 0.9rem; }
  .bar-label { color: var(--muted, #64748b); }
  .bar-track { height: 0.6rem; background: var(--c-bg, #f1f5f9); border-radius: 999px; overflow: hidden; }
  .bar-fill { height: 100%; border-radius: 999px; min-width: 2px; transition: width 200ms ease; }
  .bar-fill.concordo { background: #22c55e; }
  .bar-fill.neutro { background: #f59e0b; }
  .bar-fill.discordo { background: #ef4444; }
  .bar-fill.vote-bar { background: var(--c-green-dark, #15803d); }
  .bar-val { font-variant-numeric: tabular-nums; }
  .total { margin: 0.2rem 0 0; }
  .small { font-size: 0.82rem; }
  .submit-bar { position: sticky; bottom: 1rem; margin-top: 1.5rem; display: flex; flex-direction: column; align-items: flex-end; gap: 0.5rem; }
  .btn { display: inline-block; padding: 0.65rem 1.2rem; border-radius: 8px; border: 1px solid var(--c-ink, #0f172a); background: var(--c-paper, #fff); font-weight: 600; text-decoration: none; color: inherit; cursor: pointer; }
  .btn.primary { background: var(--c-green-dark, #15803d); border-color: var(--c-green-dark, #15803d); color: #fff; }
  .btn:disabled { opacity: 0.55; cursor: not-allowed; }
  .hint-error { color: #b91c1c; margin: 0; }
  .hint-ok { color: #15803d; margin: 0; }
  .back { margin-top: 2rem; }
</style>
