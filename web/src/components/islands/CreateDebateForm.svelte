<script lang="ts">
  // Abrir um novo debate (título + enunciado). Gated no login (dsoc_citizen);
  // o backend exige e-mail confirmado. Espelha o padrão do ProposeForm.
  import { onMount } from 'svelte';
  import { createDebate } from '../../lib/api';

  let open = $state(false);
  let loggedIn = $state(false);
  let title = $state('');
  let framing = $state('');
  let busy = $state(false);
  let msg = $state<string | null>(null);

  let valid = $derived(title.trim().length >= 8 && framing.trim().length >= 10);

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    if (!valid || busy) return;
    busy = true;
    msg = null;
    const res = await createDebate(title.trim(), framing.trim());
    busy = false;
    if (res.success && res.data) {
      window.location.href = `/debate/?id=${res.data.id}`;
    } else {
      msg = res.error?.message ?? 'Não foi possível abrir o debate.';
    }
  }

  onMount(() => {
    try {
      loggedIn = Boolean(localStorage.getItem('dsoc_citizen'));
    } catch {
      loggedIn = false;
    }
  });
</script>

<div class="wrap">
  {#if !open}
    <button type="button" class="btn-open" onclick={() => (open = true)}>
      + Abrir um debate
    </button>
  {:else if !loggedIn}
    <div class="card note">
      <p>Entre na sua conta para abrir um debate.</p>
      <a class="btn-primary" href={`/entrar/?next=${encodeURIComponent('/debates')}`}>Entrar</a>
    </div>
  {:else}
    <form class="card form" onsubmit={submit}>
      <label>
        <span>Tema do debate</span>
        <input type="text" bind:value={title} maxlength="200" placeholder="Ex.: Transporte público deve ser gratuito?" />
      </label>
      <label>
        <span>Enunciado — o que está em jogo</span>
        <textarea bind:value={framing} rows="4" maxlength="20000" placeholder="Descreva o dilema, os lados, o contexto…"></textarea>
      </label>
      {#if msg}<p class="hint-error" role="alert">{msg}</p>{/if}
      <div class="actions">
        <button type="submit" class="btn-primary" disabled={busy || !valid}>
          {busy ? 'Abrindo…' : 'Abrir debate'}
        </button>
        <button type="button" class="btn-ghost" onclick={() => (open = false)}>Cancelar</button>
      </div>
    </form>
  {/if}
</div>

<style>
  .wrap { margin-bottom: 2rem; }
  .btn-open {
    padding: 0.55rem 1.2rem;
    border-radius: 8px;
    border: 1px solid var(--border-subtle, #ccc);
    background: var(--surface-1, #fff);
    color: var(--text-1, inherit);
    font-weight: 600;
    cursor: pointer;
  }
  .card {
    background: var(--surface-1, #fff);
    border: 1px solid var(--border-subtle, rgba(0,0,0,0.1));
    border-radius: 12px;
    padding: 1.2rem;
  }
  .note { display: grid; gap: 0.7rem; justify-items: start; }
  .form { display: grid; gap: 0.85rem; }
  label { display: grid; gap: 0.3rem; font-weight: 600; font-size: 0.9rem; }
  input, textarea {
    padding: 0.6rem 0.7rem;
    border-radius: 8px;
    border: 1px solid var(--border-subtle, #ccc);
    background: var(--surface-1, #fff);
    color: var(--text-1, inherit);
    font: inherit;
    resize: vertical;
  }
  .actions { display: flex; gap: 0.6rem; }
  .btn-primary {
    padding: 0.55rem 1.2rem;
    border-radius: 8px;
    border: none;
    background: var(--accent, #15803d);
    color: var(--accent-contrast, #fff);
    font-weight: 700;
    cursor: pointer;
    text-decoration: none;
    display: inline-block;
  }
  .btn-primary:disabled { opacity: 0.5; cursor: not-allowed; }
  .btn-ghost {
    padding: 0.55rem 1.1rem;
    border-radius: 8px;
    border: 1px solid var(--border-subtle, #ccc);
    background: transparent;
    color: var(--text-1, inherit);
    font-weight: 600;
    cursor: pointer;
  }
  .hint-error { color: #dc2626; margin: 0; }
</style>
