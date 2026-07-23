<script lang="ts">
  // Editor de templates de e-mail — estilo Odoo, mas minimalista. Lista de
  // templates à esquerda, form de edição à direita. Preview no rodapé.
  //
  // Variáveis são placeholders {{name}} — clique num chip pra colar no cursor
  // do textarea ativo (subject ou body). Botão "Voltar ao padrão" restaura
  // default_subject/default_body via PATCH {reset:true}.
  import { onMount } from 'svelte';
  import {
    listEmailTemplates,
    updateEmailTemplate,
    previewEmailTemplate,
    type EmailTemplateDto,
  } from '../../lib/api';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';
  import Alert from '../ui/Alert.svelte';
  import Skeleton from '../ui/Skeleton.svelte';

  let templates = $state<EmailTemplateDto[]>([]);
  let selectedKey = $state<string | null>(null);
  let selected = $derived(templates.find((t) => t.key === selectedKey) ?? null);

  let draftSubject = $state('');
  let draftBody = $state('');
  let dirty = $derived(
    selected != null &&
      (draftSubject !== selected.subject || draftBody !== selected.body),
  );

  let loading = $state(true);
  let loadError = $state<string | null>(null);
  let saving = $state(false);
  let msg = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);

  // Preview state.
  let previewCtx = $state<Record<string, string>>({});
  let previewOut = $state<{ subject: string; body: string } | null>(null);
  let previewing = $state(false);

  // Qual textarea recebe a variável quando o admin clica num chip.
  let focused = $state<'subject' | 'body'>('body');
  let subjectRef: HTMLInputElement | null = null;
  let bodyRef: HTMLTextAreaElement | null = null;

  async function refresh() {
    loading = true;
    const res = await listEmailTemplates();
    loading = false;
    if (res.success && res.data) {
      templates = res.data;
      if (!selectedKey && templates.length > 0) select(templates[0].key);
    } else {
      loadError =
        res.error?.message ?? 'Não foi possível carregar os templates. Você é admin?';
    }
  }

  function select(key: string) {
    selectedKey = key;
    const t = templates.find((x) => x.key === key);
    if (t) {
      draftSubject = t.subject;
      draftBody = t.body;
      // Contexto de preview: pré-popula com nomes das variáveis pra o admin
      // ver a "forma" no primeiro clique.
      const ctx: Record<string, string> = {};
      for (const v of t.variables) ctx[v] = `<${v}>`;
      previewCtx = ctx;
      previewOut = null;
      msg = null;
    }
  }

  function insertVar(name: string) {
    const token = `{{${name}}}`;
    if (focused === 'subject' && subjectRef) {
      const el = subjectRef;
      const start = el.selectionStart ?? draftSubject.length;
      const end = el.selectionEnd ?? draftSubject.length;
      draftSubject =
        draftSubject.slice(0, start) + token + draftSubject.slice(end);
      queueMicrotask(() => {
        el.focus();
        el.setSelectionRange(start + token.length, start + token.length);
      });
    } else if (bodyRef) {
      const el = bodyRef;
      const start = el.selectionStart ?? draftBody.length;
      const end = el.selectionEnd ?? draftBody.length;
      draftBody = draftBody.slice(0, start) + token + draftBody.slice(end);
      queueMicrotask(() => {
        el.focus();
        el.setSelectionRange(start + token.length, start + token.length);
      });
    }
  }

  async function save() {
    if (!selected || saving) return;
    saving = true;
    msg = null;
    const res = await updateEmailTemplate(selected.key, {
      subject: draftSubject,
      body: draftBody,
    });
    saving = false;
    if (res.success) {
      msg = { kind: 'ok', text: 'Salvo.' };
      selected.subject = draftSubject;
      selected.body = draftBody;
      await refresh();
    } else {
      msg = {
        kind: 'error',
        text: res.error?.message ?? 'Falha ao salvar.',
      };
    }
  }

  async function resetToDefault() {
    if (!selected || saving) return;
    if (!confirm(`Voltar "${selected.label}" ao texto padrão?`)) return;
    saving = true;
    msg = null;
    const res = await updateEmailTemplate(selected.key, { reset: true });
    saving = false;
    if (res.success) {
      msg = { kind: 'ok', text: 'Restaurado.' };
      await refresh();
      if (selected) select(selected.key);
    } else {
      msg = {
        kind: 'error',
        text: res.error?.message ?? 'Falha ao restaurar.',
      };
    }
  }

  async function doPreview() {
    if (!selected || previewing) return;
    previewing = true;
    const res = await previewEmailTemplate(selected.key, {
      context: previewCtx,
      subject: draftSubject,
      body: draftBody,
    });
    previewing = false;
    if (res.success && res.data) previewOut = res.data;
  }

  onMount(refresh);
</script>

<Card>
  <header class="hd">
    <div>
      <h2>Templates de e-mail</h2>
      <p class="muted small">
        Edite subject/body dos e-mails que a plataforma dispara. Use
        <code>{'{{variavel}}'}</code> pros valores dinâmicos.
      </p>
    </div>
  </header>

  {#if loading}
    <Skeleton width="60%" />
    <Skeleton width="40%" />
  {:else if loadError}
    <Alert tone="danger">{loadError}</Alert>
  {:else if templates.length === 0}
    <p class="muted">Sem templates cadastrados.</p>
  {:else}
    <div class="split">
      <aside class="list" aria-label="Templates">
        {#each templates as t}
          <button
            type="button"
            class="row"
            class:active={selectedKey === t.key}
            onclick={() => select(t.key)}
          >
            <strong>{t.label}</strong>
            <span class="muted small">{t.key}</span>
          </button>
        {/each}
      </aside>

      <section class="edit">
        {#if selected}
          <div class="row-vars">
            <span class="muted small">Variáveis:</span>
            {#each selected.variables as v}
              <button
                type="button"
                class="chip"
                onclick={() => insertVar(v)}
                title={`Inserir {{${v}}} no cursor`}
              >
                {`{{${v}}}`}
              </button>
            {/each}
          </div>

          <label class="field">
            <span>Assunto</span>
            <input
              bind:this={subjectRef}
              bind:value={draftSubject}
              onfocus={() => (focused = 'subject')}
              class="input"
              type="text"
              placeholder="(vazio = usa o padrão)"
            />
          </label>

          <label class="field">
            <span>Corpo</span>
            <textarea
              bind:this={bodyRef}
              bind:value={draftBody}
              onfocus={() => (focused = 'body')}
              class="input body"
              rows="14"
              placeholder="(vazio = usa o padrão)"
            ></textarea>
          </label>

          <div class="actions">
            <Button
              variant="primary"
              onclick={save}
              disabled={!dirty || saving}
              loading={saving}
            >
              Salvar
            </Button>
            <Button variant="ghost" onclick={doPreview} loading={previewing}>
              Prévia
            </Button>
            <span class="spacer"></span>
            <Button variant="ghost" onclick={resetToDefault} disabled={saving}>
              Voltar ao padrão
            </Button>
          </div>

          {#if msg}
            <div class="alert-slot">
              <Alert tone={msg.kind === 'ok' ? 'success' : 'danger'}>
                {msg.text}
              </Alert>
            </div>
          {/if}

          {#if previewOut}
            <div class="preview">
              <h4>Prévia</h4>
              <p class="muted small">
                Valores dos placeholders são <code>&lt;nome_da_variavel&gt;</code>
                — quando o e-mail sair de verdade, cada crate injeta o valor real.
              </p>
              <div class="preview-frame">
                <p class="subject">
                  <strong>Assunto:</strong>
                  {previewOut.subject}
                </p>
                <pre class="body-pre">{previewOut.body}</pre>
              </div>
            </div>
          {/if}
        {:else}
          <p class="muted">Selecione um template à esquerda.</p>
        {/if}
      </section>
    </div>
  {/if}
</Card>

<style>
  .hd {
    margin-bottom: var(--sp-4);
  }
  h2 {
    margin: 0 0 var(--sp-1);
    font-size: var(--fs-xl);
    color: var(--text-1);
  }
  .small {
    font-size: var(--fs-sm);
  }
  code {
    background: var(--surface-2);
    padding: 1px 6px;
    border-radius: var(--r-sm);
    font-size: 0.9em;
  }
  .split {
    display: grid;
    grid-template-columns: 260px 1fr;
    gap: var(--sp-4);
  }
  @media (max-width: 720px) {
    .split {
      grid-template-columns: 1fr;
    }
  }
  .list {
    display: grid;
    gap: 6px;
    align-content: start;
  }
  .list .row {
    display: grid;
    gap: 2px;
    text-align: left;
    padding: var(--sp-3);
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-sm);
    cursor: pointer;
    color: var(--text-1);
    font-family: inherit;
  }
  .list .row:hover {
    background: var(--surface-2);
  }
  .list .row.active {
    background: var(--accent-soft);
    border-color: var(--accent);
  }
  .edit {
    display: grid;
    gap: var(--sp-3);
  }
  .row-vars {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    align-items: center;
  }
  .chip {
    padding: 3px 10px;
    background: var(--surface-2);
    border: 1px solid var(--border-subtle);
    border-radius: 999px;
    font-family: ui-monospace, monospace;
    font-size: 12px;
    cursor: pointer;
  }
  .chip:hover {
    background: var(--accent-soft);
    border-color: var(--accent);
  }
  .field {
    display: grid;
    gap: 4px;
  }
  .field > span {
    font-size: var(--fs-sm);
    color: var(--text-2);
    font-weight: var(--fw-semibold);
  }
  .input {
    width: 100%;
    padding: var(--sp-3);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-sm);
    font-family: inherit;
    font-size: var(--fs-md);
    background: var(--surface-1);
    color: var(--text-1);
  }
  .body {
    font-family: ui-monospace, monospace;
    font-size: 14px;
    line-height: 1.5;
    resize: vertical;
  }
  .actions {
    display: flex;
    gap: var(--sp-2);
    align-items: center;
  }
  .spacer {
    flex: 1;
  }
  .alert-slot {
    margin-top: var(--sp-1);
  }
  .preview {
    margin-top: var(--sp-3);
    padding-top: var(--sp-3);
    border-top: 1px dashed var(--border-subtle);
  }
  .preview h4 {
    margin: 0 0 var(--sp-2);
    font-size: var(--fs-md);
  }
  .preview-frame {
    background: var(--surface-2);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-sm);
    padding: var(--sp-3);
    margin-top: var(--sp-2);
  }
  .subject {
    margin: 0 0 var(--sp-2);
  }
  .body-pre {
    margin: 0;
    white-space: pre-wrap;
    font-family: ui-monospace, monospace;
    font-size: 13px;
    line-height: 1.5;
  }
</style>
