<script lang="ts">
  // Formulário de criação de consulta — visível só para admin ou político
  // (dsoc_is_admin / dsoc_is_politico). O backend reforça o gate (403 caso
  // contrário). createConsulta passa por apiPost → shape { success, error }.
  import { onMount } from 'svelte';
  import { createConsulta } from '../../lib/api';

  let canManage = $state(false);
  let open = $state(false);
  let busy = $state(false);
  let error = $state<string | null>(null);

  let title = $state('');
  let closesAt = $state(''); // datetime-local
  let questions = $state<string[]>(['']);

  function addQuestion() {
    questions = [...questions, ''];
  }
  function removeQuestion(i: number) {
    questions = questions.filter((_, idx) => idx !== i);
    if (questions.length === 0) questions = [''];
  }

  function defaultCloses(): string {
    // 14 dias à frente, formato datetime-local (sem timezone).
    const d = new Date(Date.now() + 14 * 24 * 60 * 60 * 1000);
    const pad = (n: number) => String(n).padStart(2, '0');
    return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
  }

  async function submit() {
    error = null;
    const cleanQ = questions.map((q) => q.trim()).filter((q) => q.length > 0);
    if (!title.trim()) {
      error = 'Informe um título.';
      return;
    }
    if (cleanQ.length === 0) {
      error = 'Adicione ao menos uma pergunta.';
      return;
    }
    if (!closesAt) {
      error = 'Defina a data de encerramento.';
      return;
    }
    const closes = new Date(closesAt);
    if (Number.isNaN(closes.getTime()) || closes.getTime() <= Date.now()) {
      error = 'O encerramento deve ser no futuro.';
      return;
    }
    busy = true;
    const res = await createConsulta({
      title: title.trim(),
      opens_at: new Date().toISOString(),
      closes_at: closes.toISOString(),
      questions: cleanQ,
    });
    busy = false;
    if (res.success && res.data) {
      window.location.href = `/consulta/?id=${res.data.id}`;
    } else {
      error = res.error?.message ?? 'Não foi possível criar a consulta.';
    }
  }

  onMount(() => {
    try {
      canManage =
        localStorage.getItem('dsoc_is_admin') === '1' ||
        localStorage.getItem('dsoc_is_politico') === '1';
    } catch {
      canManage = false;
    }
    closesAt = defaultCloses();
  });
</script>

{#if canManage}
  <div class="wrap">
    {#if !open}
      <button class="btn open-btn" onclick={() => (open = true)}>+ Abrir nova consulta</button>
    {:else}
      <form class="card form" onsubmit={(e) => { e.preventDefault(); submit(); }}>
        <h2>Nova consulta pública</h2>

        <label>
          <span>Título</span>
          <input type="text" bind:value={title} maxlength="200" placeholder="Ex.: Prioridades do orçamento 2027" />
        </label>

        <label>
          <span>Encerra em</span>
          <input type="datetime-local" bind:value={closesAt} />
        </label>

        <fieldset class="questions">
          <legend>Perguntas <span class="muted small">(o cidadão responde concordo / neutro / discordo)</span></legend>
          {#each questions as _q, i (i)}
            <div class="q-row">
              <input type="text" bind:value={questions[i]} maxlength="500" placeholder={`Pergunta ${i + 1}`} />
              {#if questions.length > 1}
                <button type="button" class="remove" onclick={() => removeQuestion(i)} aria-label="Remover pergunta">×</button>
              {/if}
            </div>
          {/each}
          <button type="button" class="add" onclick={addQuestion}>+ Adicionar pergunta</button>
        </fieldset>

        {#if error}<p class="hint-error" role="alert">{error}</p>{/if}

        <div class="actions">
          <button type="button" class="btn ghost" onclick={() => (open = false)}>Cancelar</button>
          <button type="submit" class="btn primary" disabled={busy}>{busy ? 'Criando…' : 'Publicar consulta'}</button>
        </div>
      </form>
    {/if}
  </div>
{/if}

<style>
  .wrap { margin-bottom: 2rem; }
  .card { background: var(--surface-1, var(--c-paper)); border: 1px solid var(--border-subtle, var(--c-border)); border-radius: 12px; padding: 1.5rem; }
  .form { display: grid; gap: 1rem; max-width: 44rem; }
  .form h2 { margin: 0; }
  label { display: grid; gap: 0.35rem; font-weight: 600; }
  input { padding: 0.6rem 0.7rem; border: 1px solid var(--c-border, #cbd5e1); border-radius: 8px; font: inherit; }
  .questions { border: none; padding: 0; margin: 0; display: grid; gap: 0.6rem; }
  legend { font-weight: 600; padding: 0; margin-bottom: 0.3rem; }
  .q-row { display: flex; gap: 0.5rem; align-items: center; }
  .q-row input { flex: 1; }
  .remove { width: 2.2rem; height: 2.2rem; border-radius: 8px; border: 1px solid var(--c-border, #cbd5e1); background: var(--c-paper, #fff); font-size: 1.2rem; cursor: pointer; }
  .add { justify-self: start; background: none; border: none; color: var(--c-green-dark, #15803d); font-weight: 600; cursor: pointer; padding: 0.2rem 0; }
  .actions { display: flex; gap: 0.75rem; justify-content: flex-end; }
  .btn { padding: 0.65rem 1.2rem; border-radius: 8px; border: 1px solid var(--c-ink, #0f172a); background: var(--c-paper, #fff); font-weight: 600; cursor: pointer; }
  .btn.primary { background: var(--c-green-dark, #15803d); border-color: var(--c-green-dark, #15803d); color: #fff; }
  .btn.ghost { border-color: var(--c-border, #cbd5e1); }
  .btn.open-btn { border-style: dashed; }
  .btn:disabled { opacity: 0.55; cursor: not-allowed; }
  .hint-error { color: #b91c1c; margin: 0; }
  .small { font-weight: 400; font-size: 0.82rem; }
</style>
