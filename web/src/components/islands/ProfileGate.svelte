<script lang="ts">
  // Gate obrigatório de "complete seu cadastro" (0664). Para usuários que se cadastraram antes de
  // nome/sexo/nascimento serem capturados: bloqueia a tela até completarem. Silencioso pra quem
  // já está completo ou não está logado (getProfileStatus responde 401 → não mostra nada).
  import { onMount } from 'svelte';
  import { getProfileStatus, completeProfile } from '../../lib/api';
  import { toast } from '../../lib/toasts';

  let show = $state(false);
  let offerNick = $state(false);
  let nome = $state('');
  let sexo = $state<'' | 'M' | 'F'>('');
  let nascimento = $state('');
  let nick = $state('');
  let busy = $state(false);

  const nomeValid = $derived(nome.trim().split(/\s+/).filter(Boolean).length >= 2);
  const sexoValid = $derived(sexo === 'M' || sexo === 'F');
  const nascValid = $derived(/^\d{4}-\d{2}-\d{2}$/.test(nascimento));
  const nickValid = $derived(
    !offerNick || nick.trim() === '' || /^[a-z][a-z0-9_]{2,29}$/.test(nick.trim().toLowerCase()),
  );
  const valid = $derived(nomeValid && sexoValid && nascValid && nickValid);

  onMount(async () => {
    const res = await getProfileStatus();
    if (res.success && res.data && !res.data.complete) {
      show = true;
      offerNick = res.data.auto_handle;
    }
  });

  async function save(e: Event) {
    e.preventDefault();
    if (!valid || busy) return;
    busy = true;
    const res = await completeProfile({
      nome: nome.trim(),
      sexo,
      nascimento,
      handle: offerNick && nick.trim() ? nick.trim().toLowerCase().replace(/^@/, '') : undefined,
    });
    busy = false;
    if (!res.success) {
      toast.error(res.error?.message ?? 'Não foi possível salvar.');
      return;
    }
    toast.success('Cadastro completo!');
    show = false;
    // Recarrega pra que nome/handle novos apareçam em todo lugar.
    window.location.reload();
  }
</script>

{#if show}
  <div class="gate" role="dialog" aria-modal="true" aria-labelledby="gate-h">
    <div class="card">
      <h2 id="gate-h">Complete seu cadastro</h2>
      <p class="muted">
        Precisamos de alguns dados obrigatórios pra sua conta cívica. Leva menos de um minuto.
      </p>
      <form onsubmit={save}>
        <label>
          <span>Nome completo</span>
          <input bind:value={nome} placeholder="Nome e sobrenome" required />
          {#if nome && !nomeValid}<em>Informe nome e sobrenome.</em>{/if}
        </label>
        <div class="row">
          <label>
            <span>Data de nascimento</span>
            <input type="date" bind:value={nascimento} required />
          </label>
          <label>
            <span>Sexo</span>
            <select bind:value={sexo} required>
              <option value="" disabled>Selecione</option>
              <option value="F">Feminino</option>
              <option value="M">Masculino</option>
            </select>
          </label>
        </div>
        {#if offerNick}
          <label>
            <span>Nick de usuário (@ no fediverso) — opcional</span>
            <input bind:value={nick} placeholder="ex.: maria_silva (em branco = manter o atual)" />
            {#if nick && !nickValid}<em>3–30 caracteres: a–z, 0–9, _, começando por letra.</em>{/if}
          </label>
        {/if}
        <button class="btn" disabled={!valid || busy}>
          {busy ? 'Salvando…' : 'Salvar e continuar'}
        </button>
      </form>
    </div>
  </div>
{/if}

<style>
  .gate {
    position: fixed;
    inset: 0;
    z-index: 9999;
    display: grid;
    place-items: center;
    background: rgba(0, 0, 0, 0.55);
    padding: var(--sp-4);
  }
  .card {
    width: min(30rem, 100%);
    background: var(--surface-1, #fff);
    border-radius: var(--r-base, 12px);
    padding: var(--sp-5, 1.5rem);
    box-shadow: var(--shadow-lg, 0 10px 40px rgba(0, 0, 0, 0.3));
    display: grid;
    gap: var(--sp-3, 0.75rem);
  }
  h2 {
    margin: 0;
    color: var(--text-1);
  }
  .muted {
    margin: 0;
    color: var(--text-3);
    font-size: var(--fs-sm);
  }
  form {
    display: grid;
    gap: var(--sp-3, 0.75rem);
    margin-top: var(--sp-2);
  }
  label {
    display: grid;
    gap: 4px;
    font-size: var(--fs-sm);
    font-weight: var(--fw-semibold);
    color: var(--text-1);
  }
  input,
  select {
    font: inherit;
    font-weight: 400;
    padding: var(--sp-3, 0.7rem);
    border: 1px solid var(--border-subtle, #cbd5e1);
    border-radius: var(--r-sm, 8px);
    background: var(--surface-1, #fff);
    color: var(--text-1);
  }
  .row {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: var(--sp-3, 0.75rem);
  }
  em {
    color: #dc2626;
    font-style: normal;
    font-weight: 400;
    font-size: var(--fs-xs);
  }
  .btn {
    margin-top: var(--sp-2);
    padding: var(--sp-3) var(--sp-5);
    border: none;
    border-radius: var(--r-sm, 8px);
    background: var(--accent, #15803d);
    color: #fff;
    font-weight: var(--fw-semibold);
    cursor: pointer;
  }
  .btn:disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }
</style>
