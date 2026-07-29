<script lang="ts">
  // Propose form — create a civic proposal directed at a specific mandate.
  // The mandate is selected from a picker so the user never has to type a UUID by hand.
  import { onMount } from 'svelte';
  import {
    createForumTopic,
    browsePoliticos,
    DEFAULT_ORG_ID,
    getAllMandates,
    getThresholdPreview,
    listMunicipios,
    type MandateDto,
    type MunicipioRow,
    type PoliticoRow,
    type ThresholdPreviewDto,
  } from '../../lib/api';
  import { UFS } from '../../lib/ufs';

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

  // Esfera municipal (follow-up do 0537): 68k mandatos municipais não cabem num
  // dropdown — cascata UF → município carrega só os vereadores/prefeito do lugar.
  // Federal + estadual continuam no dropdown único (lista cacheada).
  let esfera = $state<'federal-estadual' | 'municipal'>('federal-estadual');
  let fedEstMandates: MandateDto[] = [];
  let ufSel = $state('');
  let municipioSel = $state('');
  let municipios = $state<MunicipioRow[]>([]);
  let municipiosLoading = $state(false);

  // browse retorna PoliticoRow; o form trabalha com o shape MandateDto.
  function asMandate(r: PoliticoRow): MandateDto {
    return {
      id: r.id,
      office: r.office,
      display_name: r.display_name,
      is_candidate: r.is_candidate,
      onboarded: false,
      party: r.party,
      uf: r.uf,
      house: (r.house as MandateDto['house']) ?? null,
      avatar_url: r.avatar_url,
      sphere: r.sphere,
    };
  }

  function switchEsfera(next: 'federal-estadual' | 'municipal') {
    if (esfera === next) return;
    esfera = next;
    mandateId = '';
    extraIds = [];
    coSearch = '';
    if (next === 'federal-estadual') {
      mandates = fedEstMandates;
      mandatesError = null;
    } else {
      mandates = [];
      ufSel = '';
      municipioSel = '';
      municipios = [];
    }
  }

  async function onUfChange() {
    municipioSel = '';
    mandates = [];
    mandateId = '';
    extraIds = [];
    municipios = [];
    if (!ufSel) return;
    municipiosLoading = true;
    const res = await listMunicipios(ufSel);
    municipiosLoading = false;
    if (res.success && res.data) {
      municipios = res.data;
    } else {
      mandatesError = 'Não foi possível carregar os municípios dessa UF.';
    }
  }

  async function onMunicipioChange() {
    mandates = [];
    mandateId = '';
    extraIds = [];
    if (!ufSel || !municipioSel) return;
    mandatesLoading = true;
    const res = await browsePoliticos({
      sphere: 'municipal',
      uf: ufSel,
      municipio: municipioSel,
      limit: 200,
    });
    mandatesLoading = false;
    if (res.success && res.data) {
      mandates = res.data.items.map(asMandate);
      mandatesError = null;
      if (mandates.length === 1) mandateId = mandates[0].id;
    } else {
      mandatesError =
        res.error?.message ?? 'Não foi possível carregar os políticos do município.';
    }
  }

  // Multi-destinatário (0537): co-destinatários da MESMA esfera do principal.
  // O servidor valida de novo; aqui o filtro é só UX.
  const MAX_TARGETS = 10;
  let extraIds = $state<string[]>([]);
  let coSearch = $state('');

  let primary = $derived(mandates.find((m) => m.id === mandateId) ?? null);
  let primarySphere = $derived(primary?.sphere ?? 'federal');
  let coCandidates = $derived(
    primary
      ? mandates.filter(
          (m) =>
            m.id !== primary.id &&
            (m.sphere ?? 'federal') === primarySphere &&
            (coSearch.trim().length < 2 ||
              m.display_name
                .toLocaleLowerCase('pt-BR')
                .includes(coSearch.trim().toLocaleLowerCase('pt-BR'))),
        )
      : [],
  );
  let extras = $derived(
    extraIds
      .map((id) => mandates.find((m) => m.id === id))
      .filter((m): m is MandateDto => Boolean(m)),
  );

  // Troca de principal (ou de esfera): descarta co-destinatários incompatíveis.
  // Só reatribui quando algo mudou — atribuição incondicional re-dispararia o effect.
  $effect(() => {
    const sphere = primarySphere;
    const pid = mandateId;
    const kept = extraIds.filter((id) => {
      if (id === pid) return false;
      const m = mandates.find((x) => x.id === id);
      return m ? (m.sphere ?? 'federal') === sphere : false;
    });
    if (kept.length !== extraIds.length) extraIds = kept;
  });

  function toggleExtra(id: string) {
    if (extraIds.includes(id)) {
      extraIds = extraIds.filter((x) => x !== id);
    } else if (extraIds.length < MAX_TARGETS - 1) {
      extraIds = [...extraIds, id];
    }
  }

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
      fedEstMandates = res.data;
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

  // Slug de município → segmento de caminho ("São Paulo" → "sao-paulo"), MESMA
  // regra do backend (domain::slugify / seed SQL): a demanda direcionada vira um
  // tópico no fórum territorial do gabinete.
  function slugify(name: string): string {
    return name
      .normalize('NFD')
      .replace(/[̀-ͯ]/g, '')
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/^-+|-+$/g, '');
  }

  // Fórum-anfitrião da demanda direcionada (B1): a régua/patamar é do fórum, o
  // encaminhamento vai aos gabinetes-alvo. Territorial pelo destinatário:
  // municipal → <uf>/<municipio>; estadual → <uf>; federal → senado|camara.
  function targetForumPath(): string {
    if (esfera === 'municipal' && ufSel && municipioSel) {
      return `${ufSel.toLowerCase()}/${slugify(municipioSel)}`;
    }
    const m = primary;
    const sphere = m?.sphere ?? 'federal';
    if (sphere === 'municipal' && m?.uf) return m.uf.toLowerCase();
    if (sphere === 'estadual' && m?.uf) return m.uf.toLowerCase();
    if (sphere === 'federal') return m?.house === 'senado' ? 'senado' : 'camara';
    return m?.uf ? m.uf.toLowerCase() : 'camara';
  }

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!valid || busy) return;
    status = null;

    if (!readCitizenId()) {
      status = {
        kind: 'info',
        text: 'Entre na sua conta para enviar uma demanda.',
      };
      return;
    }

    busy = true;
    // Uma porta, uma régua (B1): a "proposta" agora é um TÓPICO DE FÓRUM
    // direcionado ao(s) gabinete(s). Mesmo placar por pontos e patamar
    // proporcional do fórum; o alvo só decide para onde vai o encaminhamento.
    const targets = [mandateId.trim(), ...extraIds];
    const res = await createForumTopic(
      targetForumPath(),
      title.trim(),
      body.trim(),
      targets,
    );
    busy = false;

    if (res.success && res.data) {
      window.location.href = `/f/topico/${res.data.id}`;
    } else {
      status = {
        kind: 'error',
        text: res.error?.message ?? 'Não foi possível enviar a demanda.',
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

  <div class="field">
    <span class="label-like">Nível do destinatário</span>
    <div class="esfera-toggle" role="radiogroup" aria-label="Nível do destinatário">
      <label>
        <input
          type="radio"
          name="esfera"
          checked={esfera === 'federal-estadual'}
          onchange={() => switchEsfera('federal-estadual')}
        />
        Federal e Estadual
      </label>
      <label>
        <input
          type="radio"
          name="esfera"
          checked={esfera === 'municipal'}
          onchange={() => switchEsfera('municipal')}
        />
        Municipal (vereadores e prefeitos)
      </label>
    </div>
  </div>

  {#if esfera === 'municipal'}
    <div class="row row-2">
      <div class="field">
        <label for="p-uf">Estado (UF)</label>
        <select id="p-uf" class="input" bind:value={ufSel} onchange={onUfChange}>
          <option value="" disabled>Escolha a UF…</option>
          {#each UFS as u (u.code)}
            <option value={u.code}>{u.name}</option>
          {/each}
        </select>
      </div>
      <div class="field">
        <label for="p-municipio">Município</label>
        {#if municipiosLoading}
          <p class="hint muted">Carregando municípios…</p>
        {:else}
          <select
            id="p-municipio"
            class="input"
            bind:value={municipioSel}
            onchange={onMunicipioChange}
            disabled={!ufSel}
          >
            <option value="" disabled>
              {ufSel ? 'Escolha o município…' : 'Escolha a UF primeiro'}
            </option>
            {#each municipios as m (m.nome)}
              <option value={m.nome}>{m.nome}</option>
            {/each}
          </select>
        {/if}
      </div>
    </div>
  {/if}

  <div class="row">
    <div class="field">
      <label for="p-mandate">Político destinatário</label>
      {#if esfera === 'municipal' && !municipioSel}
        <p class="hint muted">Escolha a UF e o município acima.</p>
      {:else if mandatesLoading}
        <p class="hint muted">Carregando lista de políticos…</p>
      {:else if mandatesError}
        <p class="hint hint-error">{mandatesError}</p>
      {:else if mandates.length === 0}
        <p class="hint muted">
          {esfera === 'municipal'
            ? 'Nenhum vereador ou prefeito cadastrado neste município.'
            : 'Ainda não há políticos cadastrados nesta plataforma.'}
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
        <span class="label-like">Quantos apoios são necessários aqui</span>
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

  {#if primary}
    <div class="field co-block">
      <span class="label-like">
        Enviar também para outros gabinetes
          <span class="muted">
            (opcional — mesmo nível: {primarySphere === 'federal'
              ? 'deputados federais e senadores'
              : primarySphere === 'estadual'
                ? 'deputados estaduais e governadores'
                : 'vereadores e prefeitos'})
          </span>
        </span>
        {#if extras.length > 0}
          <ul class="co-chips">
            {#each extras as m (m.id)}
              <li>
                <button
                  type="button"
                  class="co-chip"
                  title="Remover destinatário"
                  onclick={() => toggleExtra(m.id)}
                >
                  {m.display_name} ✕
                </button>
              </li>
            {/each}
          </ul>
        {/if}
        {#if extraIds.length >= MAX_TARGETS - 1}
          <p class="hint muted">
            Limite de {MAX_TARGETS} destinatários por proposta atingido.
          </p>
        {:else}
          <input
            class="input"
            type="search"
            placeholder="Buscar político pelo nome…"
            bind:value={coSearch}
          />
          {#if coSearch.trim().length >= 2}
            {#if coCandidates.length === 0}
              <p class="hint muted">Nenhum político desse nível com esse nome.</p>
            {:else}
              <ul class="co-results">
                {#each coCandidates.slice(0, 12) as m (m.id)}
                  <li>
                    <label class="co-option">
                      <input
                        type="checkbox"
                        checked={extraIds.includes(m.id)}
                        onchange={() => toggleExtra(m.id)}
                      />
                      {m.display_name} — {m.office}{m.party ? ` (${m.party}/${m.uf ?? ''})` : ''}
                    </label>
                  </li>
                {/each}
              </ul>
            {/if}
          {/if}
        {/if}
    </div>
  {/if}

  <button class="btn btn-primary btn-lg" type="submit" disabled={!valid || busy}>
    {busy ? 'Enviando…' : 'Enviar demanda'}
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
  .esfera-toggle {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem 1.25rem;
    margin: 0.35rem 0 0.5rem;
  }
  .esfera-toggle label {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    cursor: pointer;
    font-size: 0.95rem;
  }
  @media (min-width: 640px) {
    .row-2 {
      grid-template-columns: 1fr 1fr;
    }
  }
  .co-block {
    margin: 0.75rem 0 1rem;
  }
  .co-chips {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
    list-style: none;
    margin: 0.4rem 0 0.6rem;
    padding: 0;
  }
  .co-chip {
    border: 1px solid var(--c-border, #ccc);
    border-radius: 999px;
    background: transparent;
    color: inherit;
    padding: 0.2rem 0.7rem;
    font-size: 0.88rem;
    cursor: pointer;
  }
  .co-results {
    list-style: none;
    margin: 0.4rem 0 0;
    padding: 0.25rem 0;
    max-height: 14rem;
    overflow-y: auto;
    border: 1px solid var(--c-border, #ccc);
    border-radius: 0.5rem;
  }
  .co-option {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.3rem 0.6rem;
    font-size: 0.92rem;
    cursor: pointer;
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
