<script lang="ts">
  import { onMount } from 'svelte';
  import {
    addQuestion,
    buildAnswers,
    createSurvey,
    getSurvey,
    listQuestions,
    listSurveys,
    nextPosition,
    publishSurvey,
    sortQuestions,
    submitResponse,
    YES_NO_KIND,
    type QuestionDto,
    type SurveyDto,
  } from '../../lib/surveys';
  import { toast } from '../../lib/toasts';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';

  let loading = $state(true);
  let loadError = $state<string | null>(null);
  let logged = $state(false);
  let surveys = $state<SurveyDto[]>([]);
  let current = $state<SurveyDto | null>(null);
  let questions = $state<QuestionDto[]>([]);
  let draft = $state<Record<string, string>>({});
  let answered = $state(false);
  let busy = $state(false);

  let newTitle = $state('');
  let newPrompt = $state('');
  let newKind = $state('text');

  async function loadDetail(id: string) {
    const [survey, items] = await Promise.all([getSurvey(id), listQuestions(id)]);
    if (survey.ok && survey.data) {
      current = survey.data;
      questions = sortQuestions(items.data ?? []);
    } else {
      loadError = survey.error ?? 'Enquete não encontrada.';
    }
  }

  onMount(async () => {
    try {
      logged = Boolean(localStorage.getItem('dsoc_citizen'));
    } catch {}
    const id = new URLSearchParams(window.location.search).get('id');
    if (id) {
      await loadDetail(id);
    } else {
      const res = await listSurveys();
      if (res.ok && res.data) surveys = res.data;
      else loadError = res.error ?? 'Não foi possível carregar as enquetes.';
    }
    loading = false;
  });

  async function create(event: SubmitEvent) {
    event.preventDefault();
    busy = true;
    const res = await createSurvey(newTitle);
    busy = false;
    if (res.success && res.data) window.location.href = `/enquetes/?id=${res.data.id}`;
    else toast.error(res.error?.message ?? 'Não foi possível criar a enquete.');
  }

  async function addNewQuestion(event: SubmitEvent) {
    event.preventDefault();
    if (!current) return;
    busy = true;
    const res = await addQuestion(current.id, {
      prompt: newPrompt,
      kind: newKind,
      position: nextPosition(questions),
    });
    busy = false;
    if (res.success) {
      newPrompt = '';
      await loadDetail(current.id);
    } else {
      toast.error(res.error?.message ?? 'Não foi possível adicionar a pergunta.');
    }
  }

  async function publish() {
    if (!current) return;
    busy = true;
    const res = await publishSurvey(current.id);
    busy = false;
    if (res.success) {
      toast.success('Enquete publicada.');
      await loadDetail(current.id);
    } else {
      toast.error(res.error?.message ?? 'Não foi possível publicar a enquete.');
    }
  }

  async function respond(event: SubmitEvent) {
    event.preventDefault();
    if (!current) return;
    const answers = buildAnswers(questions, draft);
    if (!answers) {
      toast.error('Responda todas as perguntas.');
      return;
    }
    busy = true;
    const res = await submitResponse(current.id, answers);
    busy = false;
    if (res.success) answered = true;
    else toast.error(res.error?.message ?? 'Não foi possível enviar suas respostas.');
  }
</script>

{#if loading}
  <p class="muted">Carregando…</p>
{:else if loadError}
  <Card><p role="alert">{loadError}</p></Card>
{:else if current}
  <p><a href="/enquetes/">← Todas as enquetes</a></p>
  <h1>{current.title}</h1>

  {#if current.published_at}
    {#if answered}
      <p role="status">Obrigado! Suas respostas foram registradas.</p>
    {:else if !logged}
      <p class="muted"><a href="/entrar">Entre</a> para responder.</p>
    {:else}
      <form onsubmit={respond}>
        {#each questions as q (q.id)}
          <label>
            {q.prompt}
            {#if q.kind === YES_NO_KIND}
              <select bind:value={draft[q.id]} required>
                <option value="">Selecione</option>
                <option value="yes">Sim</option>
                <option value="no">Não</option>
              </select>
            {:else}
              <textarea bind:value={draft[q.id]} rows="3" required></textarea>
            {/if}
          </label>
        {/each}
        <Button type="submit" disabled={busy}>Enviar respostas</Button>
      </form>
    {/if}
  {:else}
    <p class="muted">Rascunho: ainda não aceita respostas.</p>
    <ol>
      {#each questions as q (q.id)}
        <li>{q.prompt}</li>
      {/each}
    </ol>
    {#if logged}
      <form onsubmit={addNewQuestion}>
        <label>Nova pergunta <input bind:value={newPrompt} required /></label>
        <label>
          Tipo de resposta
          <select bind:value={newKind}>
            <option value="text">Texto livre</option>
            <option value={YES_NO_KIND}>Sim ou não</option>
          </select>
        </label>
        <Button type="submit" variant="ghost" disabled={busy}>Adicionar pergunta</Button>
      </form>
      <Button disabled={busy || questions.length === 0} onclick={publish}>Publicar enquete</Button>
    {/if}
  {/if}
{:else}
  <h1>Enquetes</h1>
  {#if surveys.length === 0}
    <p class="muted">Nenhuma enquete por enquanto.</p>
  {:else}
    <ul class="list">
      {#each surveys as s (s.id)}
        <li>
          <a href={`/enquetes/?id=${s.id}`}>{s.title}</a>
          <span class="muted">{s.published_at ? 'Aberta' : 'Rascunho'}</span>
        </li>
      {/each}
    </ul>
  {/if}
  {#if logged}
    <Card>
      <h2>Criar enquete</h2>
      <form onsubmit={create}>
        <label>Título <input bind:value={newTitle} required /></label>
        <Button type="submit" disabled={busy}>Criar</Button>
      </form>
    </Card>
  {/if}
{/if}

<style>
  .list {
    list-style: none;
    margin: 0 0 var(--sp-4);
    padding: 0;
  }
  .list li {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: var(--sp-2) 0;
    border-top: 1px solid var(--border-subtle);
  }
  form {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: var(--sp-3);
    margin: var(--sp-3) 0;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: var(--sp-1);
    width: 100%;
  }
</style>
