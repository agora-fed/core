<script lang="ts">
  // Propose form — create a civic proposal directed at a specific mandate.
  // The mandate is selected from a picker so the user never has to type a UUID by hand.
  import { onMount } from 'svelte';
  import {
    apiPost,
    DEFAULT_ORG_ID,
    getAllMandates,
    getThresholdPreview,
    type MandateDto,
    type ThresholdPreviewDto,
  } from '../../lib/api';
  import type { ProposalDto } from '../../lib/types';

  let title = $state('');
  let body = $state('');
  let mandateId = $state('');
  // O gatilho é POLÍTICA DA PLATAFORMA (0,05% do eleitorado do território,
  // com piso/teto) — o autor não escolhe; o form mostra o valor calculado.
  let preview = $state<ThresholdPreviewDto | null>(null);
  let busy = $state(false);
  let status = $state<{ kind: 'error' | 'ok' | 'info'; text: string } | null>(
    null,
  );

  let mandates = $state<MandateDto[]>([]);
  let mandatesLoading = $state(true);
  let mandatesError = $state<string | null>(null);

  let titleValid = $derived(title.trim().length >= 8);
  let bodyValid = $derived(body.trim().length >= 20);
  let mandateValid = $derived(/^[0-9a-f-]{36}$/i.test(mandateId.trim()));
  let valid = $derived(titleValid && bodyValid && mandateValid);

  $effect(() => {
    const id = mandateId.trim();
    if (!/^[0-9a-f-]{36}$/i.test(id)) {
      preview = null;
      return;
    }
    void getThresholdPreview(id).then((r) => {
      if (r.success && r.data) preview = r.data;
    });
  });

  function readCitizenId(): string | null {
    try {
      return localStorage.getItem('dsoc_citizen');
    } catch {
      return null;
    }
  }

  onMount(async () => {
    // Federal + estadual — proposals can target Congress and Assemblies. A
    // dropdown of 68k municipais overflows the picker; municipal is added in
    // a follow-up (search-first picker with UF/município cascade).
    const [fed, est] = await Promise.all([
      getAllMandates(DEFAULT_ORG_ID, 5000, 'federal'),
      getAllMandates(DEFAULT_ORG_ID, 5000, 'estadual'),
    ]);
    mandatesLoading = false;
    if ((fed.ok && fed.data) || (est.ok && est.data)) {
      const merged = [
        ...(fed.ok && fed.data ? fed.data : []),
        ...(est.ok && est.data ? est.data : []),
      ];
      // Reuse the same res shape below.
      const res = { ok: true as const, data: merged, error: null };
      mandates = res.data;
      // Pre-select when the URL carries `?mandate=<id>` (linked from /politicos/<id>).
      try {
        const params = new URLSearchParams(window.location.search);
        const pre = params.get('mandate');
        if (pre && mandates.some((m) => m.id === pre)) {
          mandateId = pre;
        } else if (mandates.length === 1) {
          mandateId = mandates[0].id;
        }
      } catch {
        if (mandates.length === 1) mandateId = mandates[0].id;
      }
    } else {
      mandatesError =
        res.error ?? 'Não foi possível carregar a lista de políticos.';
    }
  });

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!valid || busy) return;
    status = null;

    if (!readCitizenId()) {
      status = {
        kind: 'info',
        text: 'Entre na sua conta para enviar uma proposta.',
      };
      return;
    }

    busy = true;
    const res = await apiPost<ProposalDto>('/api/v1/proposals', {
      org_id: DEFAULT_ORG_ID,
      mandate_id: mandateId.trim(),
      title: title.trim(),
      body: body.trim(),
      // Sobrescrito no servidor pela política do território (0.30.1) —
      // enviado só pra satisfazer o shape do contrato.
      threshold: preview?.threshold ?? 1,
    });
    busy = false;

    if (res.success && res.data) {
      window.location.href = `/propostas/${res.data.id}`;
    } else {
      status = {
        kind: 'error',
        text: res.error?.message ?? 'Não foi possível enviar a proposta.',
      };
    }
  }
</script>

<form class="propose" onsubmit={submit} novalidate>
  <div class="field">
    <label for="p-title">Título da proposta</label>
    <input
      id="p-title"
      class="input"
      type="text"
      bind:value={title}
      maxlength="160"
      aria-invalid={title.length > 0 && !titleValid}
      placeholder="Ex.: Ciclovia ligando o bairro ao centro"
    />
    {#if title.length > 0 && !titleValid}
      <p class="hint hint-error">Use ao menos 8 caracteres.</p>
    {/if}
  </div>

  <div class="field">
    <label for="p-body">Descrição</label>
    <textarea
      id="p-body"
      class="input"
      rows="5"
      bind:value={body}
      aria-invalid={body.length > 0 && !bodyValid}
      placeholder="Explique o problema, quem é afetado e a mudança proposta…"
    ></textarea>
    {#if body.length > 0 && !bodyValid}
      <p class="hint hint-error">Descreva com ao menos 20 caracteres.</p>
    {/if}
  </div>

  <div class="row">
    <div class="field">
      <label for="p-mandate">Político destinatário</label>
      {#if mandatesLoading}
        <p class="hint muted">Carregando lista de políticos…</p>
      {:else if mandatesError}
        <p class="hint hint-error">{mandatesError}</p>
      {:else if mandates.length === 0}
        <p class="hint muted">
          Ainda não há políticos cadastrados nesta plataforma.
        </p>
      {:else}
        <select
          id="p-mandate"
          class="input"
          bind:value={mandateId}
          aria-invalid={mandateId.length > 0 && !mandateValid}
        >
          <option value="" disabled>Escolha um político…</option>
          {#each mandates as m (m.id)}
            <option value={m.id}>
              {m.display_name} — {m.office}{m.is_candidate
                ? ' (candidatura)'
                : ''}
            </option>
          {/each}
        </select>
      {/if}
    </div>
    {#if preview}
      <div class="field narrow">
        <span class="label-like">Gatilho deste território</span>
        <p class="threshold-info">
          🎯 <strong>{preview.threshold.toLocaleString('pt-BR')} apoios</strong>
          {#if preview.voters}
            — {(preview.fraction * 100).toLocaleString('pt-BR', {
              maximumFractionDigits: 2,
            })}% do eleitorado ({preview.voters.toLocaleString('pt-BR')} eleitores,
            fonte TSE)
          {:else}
            — piso da plataforma (território sem dado de eleitorado)
          {/if}
        </p>
      </div>
    {/if}
  </div>

  <button class="btn btn-primary btn-lg" type="submit" disabled={!valid || busy}>
    {busy ? 'Enviando…' : 'Enviar proposta'}
  </button>

  {#if status}
    <p class={`note ${status.kind}`} role="status">
      {status.text}
      {#if status.kind === 'info'}<a href="/entrar">Entrar</a>{/if}
    </p>
  {/if}
</form>

<style>
  .propose {
    display: block;
  }
  textarea.input {
    resize: vertical;
  }
  .row {
    display: grid;
    gap: 1rem;
  }
  @media (min-width: 640px) {
    .row {
      grid-template-columns: 1fr 10rem;
    }
  }
  .note {
    margin: 0.75rem 0 0;
    font-size: 0.92rem;
  }
  .note.error {
    color: var(--c-ignored);
  }
  .note.info {
    color: var(--c-text-muted);
  }
</style>
