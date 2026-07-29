<script lang="ts">
  // Painel do mandato — onde o(a) parlamentar vinculado(a) à conta atual VÊ suas SLAs e
  // RESPONDE publicamente. Sem essa página, o loop da plataforma é unilateral (cidadã cobra,
  // política nunca responde) — o silêncio público existe mas a alternativa não.
  import { onMount } from 'svelte';
  import {
    getMyMandate,
    getSlas,
    getProposal,
    getMandateCrm,
    getMandateCommitments,
    createCommitment,
    openCommitmentConsultation,
    recordCommitmentOutcome,
    respondToSla,
    DEFAULT_ORG_ID,
    type MyMandateDto,
  } from '../../lib/api';
  import type {
    SlaDto,
    ProposalDto,
    SlaStatus,
    MandateCrmDto,
    CrmStatus,
    PublicCommitment,
  } from '../../lib/types';
  import SlaClock from './SlaClock.svelte';
  import MandateOpPanel from './MandateOpPanel.svelte';

  let loading = $state(true);
  let mine = $state<MyMandateDto | null>(null);
  let slas = $state<SlaDto[]>([]);
  let proposalsById = $state<Map<string, ProposalDto>>(new Map());
  let loadError = $state<string | null>(null);

  // Per-SLA response form state.
  let openId = $state<string | null>(null);
  let body = $state('');
  let committed = $state(false);
  let busy = $state(false);
  let status = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);

  let pending = $derived(slas.filter((s) => s.status === 'pending'));
  let settled = $derived(slas.filter((s) => s.status !== 'pending'));

  // --- CRM de gabinete (C6): "quem te procurou e o que pediu" ------------------
  type Tab = 'fila' | 'crm' | 'compromissos' | 'op';
  let activeTab = $state<Tab>('fila');
  let crm = $state<MandateCrmDto | null>(null);
  let crmLoading = $state(false);
  let crmError = $state<string | null>(null);
  let crmLoaded = $state(false);
  // Filtros (recontam no servidor).
  let crmStatus = $state<'' | CrmStatus>('');
  let crmTheme = $state('');
  // Quais contatos estão expandidos (mostrando o histórico de demandas).
  let openContacts = $state<Set<string>>(new Set());

  const CRM_STATUS_LABEL: Record<CrmStatus, string> = {
    respondida: '✓ respondida',
    pendente: '⏳ prazo correndo',
    silencio: '✗ silêncio',
    aberta: '• reunindo apoios',
  };

  async function loadCrm() {
    crmLoading = true;
    crmError = null;
    const res = await getMandateCrm({
      status: crmStatus || undefined,
      theme: crmTheme || undefined,
    });
    crmLoading = false;
    crmLoaded = true;
    if (res.success && res.data) {
      crm = res.data;
    } else {
      crmError = res.error?.message ?? 'Não foi possível carregar o CRM.';
    }
  }

  function selectTab(tab: Tab) {
    activeTab = tab;
    if (tab === 'crm' && !crmLoaded) loadCrm();
    if (tab === 'compromissos' && !commitLoaded) loadCommitments();
  }

  // --- Compromissos (D8.1): compromisso consultivo VOLUNTÁRIO do mandato -------
  const OUTCOME_LABEL: Record<PublicCommitment['outcome'], string> = {
    seguiu: '🟢 seguiu a base',
    nao_seguiu: '🔴 não seguiu',
    pendente: '🕓 pendente',
  };

  let commitments = $state<PublicCommitment[]>([]);
  let commitLoading = $state(false);
  let commitLoaded = $state(false);
  let commitError = $state<string | null>(null);
  let commitMsg = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);

  // Form de novo compromisso.
  let newTheme = $state('');
  let newDescription = $state('');
  let creating = $state(false);
  // Ações por compromisso.
  let busyCommit = $state<string | null>(null);

  async function loadCommitments() {
    if (!mine?.mandate) return;
    commitLoading = true;
    commitError = null;
    const res = await getMandateCommitments(mine.mandate.id);
    commitLoading = false;
    commitLoaded = true;
    if (res.ok && res.data) {
      commitments = res.data.commitments;
    } else {
      commitError = res.error ?? 'Não foi possível carregar os compromissos.';
    }
  }

  async function submitCommitment() {
    if (creating) return;
    const theme = newTheme.trim();
    const description = newDescription.trim();
    if (!theme || !description) return;
    creating = true;
    commitMsg = null;
    const res = await createCommitment(theme, description);
    creating = false;
    if (res.success) {
      newTheme = '';
      newDescription = '';
      commitMsg = { kind: 'ok', text: 'Compromisso declarado publicamente.' };
      await loadCommitments();
    } else {
      commitMsg = {
        kind: 'error',
        text: res.error?.message ?? 'Não foi possível declarar o compromisso.',
      };
    }
  }

  async function consult(id: string) {
    if (busyCommit) return;
    busyCommit = id;
    commitMsg = null;
    const res = await openCommitmentConsultation(id);
    busyCommit = null;
    if (res.success) {
      commitMsg = { kind: 'ok', text: 'Consulta à base aberta.' };
      await loadCommitments();
    } else {
      commitMsg = {
        kind: 'error',
        text: res.error?.message ?? 'Não foi possível abrir a consulta.',
      };
    }
  }

  async function setOutcome(id: string, outcome: 'seguiu' | 'nao_seguiu') {
    if (busyCommit) return;
    const note =
      window.prompt(
        outcome === 'seguiu'
          ? 'Como o mandato seguiu a base? (opcional)'
          : 'Por que o mandato não seguiu a base? (opcional)',
      ) ?? undefined;
    busyCommit = id;
    commitMsg = null;
    const res = await recordCommitmentOutcome(id, outcome, note || undefined);
    busyCommit = null;
    if (res.success) {
      commitMsg = { kind: 'ok', text: 'Resultado registrado. Aparece no perfil público.' };
      await loadCommitments();
    } else {
      commitMsg = {
        kind: 'error',
        text: res.error?.message ?? 'Não foi possível registrar o resultado.',
      };
    }
  }

  function toggleTheme(theme: string) {
    crmTheme = crmTheme === theme ? '' : theme;
    loadCrm();
  }

  function toggleContact(id: string) {
    const next = new Set(openContacts);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    openContacts = next;
  }

  function contactName(c: MandateCrmDto['contacts'][number]): string {
    return c.display_name || `@${c.handle ?? c.public_handle}`;
  }

  function fmtDate(iso: string): string {
    try {
      return new Date(iso).toLocaleDateString('pt-BR');
    } catch {
      return iso;
    }
  }

  onMount(async () => {
    const mr = await getMyMandate();
    if (!mr.success || !mr.data) {
      loading = false;
      loadError = mr.error?.message ?? 'Não foi possível verificar seu vínculo.';
      return;
    }
    mine = mr.data;
    if (!mine.mandate) {
      loading = false;
      return;
    }
    // Pull SLAs scoped to the org, filter to MY mandate.
    const sr = await getSlas(DEFAULT_ORG_ID, 200);
    if (sr.ok && sr.data) {
      slas = sr.data.filter((s) => s.mandate_id === mine!.mandate!.id);
      // Hydrate proposal titles (one fetch per SLA, parallel).
      const titles = await Promise.all(
        slas.map((s) => getProposal(s.proposal_id)),
      );
      const map = new Map<string, ProposalDto>();
      titles.forEach((p) => {
        if (p.ok && p.data) map.set(p.data.id, p.data);
      });
      proposalsById = map;
    }
    loading = false;
  });

  function open(id: string) {
    openId = openId === id ? null : id;
    body = '';
    committed = false;
    status = null;
  }

  async function submit(slaId: string) {
    if (busy || body.trim().length < 10) return;
    busy = true;
    status = null;
    const res = await respondToSla(slaId, body.trim(), committed);
    busy = false;
    if (res.success) {
      status = {
        kind: 'ok',
        text: committed
          ? 'Resposta com compromisso registrada. Aparece no placar.'
          : 'Resposta registrada. Aparece no placar.',
      };
      // Optimistic: flip the SLA out of "pending" in the local view.
      slas = slas.map((s) =>
        s.id === slaId
          ? { ...s, status: (committed ? 'acted' : 'answered') as SlaStatus }
          : s,
      );
      openId = null;
      body = '';
      committed = false;
    } else {
      status = {
        kind: 'error',
        text: res.error?.message ?? 'Não foi possível registrar a resposta.',
      };
    }
  }
</script>

{#if loading}
  <p class="muted">Carregando…</p>
{:else if loadError}
  <p class="hint hint-error" role="alert">{loadError}</p>
{:else if !mine?.mandate}
  <div class="card center">
    <h2>Você ainda não está vinculada a um mandato</h2>
    <p class="muted">
      Esta página existe para parlamentares acompanharem e responderem
      publicamente às demandas direcionadas a eles. Se você é parlamentar,
      precisa aceitar um convite de vinculação primeiro.
    </p>
    <p class="muted">
      <a href="/configuracoes">Voltar para configurações</a>
    </p>
  </div>
{:else}
  <header class="head">
    {#if mine.mandate.avatar_url}
      <img class="avatar" src={mine.mandate.avatar_url} alt="" />
    {:else}
      <span class="avatar avatar-placeholder">👤</span>
    {/if}
    <div class="who">
      <h1>Painel do mandato</h1>
      <p class="muted">
        <strong>{mine.mandate.display_name}</strong> ·
        {mine.mandate.party}/{mine.mandate.uf} ·
        {mine.mandate.house === 'camara' ? 'Câmara' : 'Senado'}
      </p>
      <p class="muted small">
        Vínculo verificado em nível <strong>{mine.binding_level}</strong>.
      </p>
    </div>
    <a class="btn btn-ghost placar" href={`/politicos/${mine.mandate.id}/placar`}>
      📊 Ver placar público
    </a>
  </header>

  <nav class="tabs" aria-label="Seções do painel">
    <button
      type="button"
      class={`tab ${activeTab === 'fila' ? 'is-active' : ''}`}
      onclick={() => selectTab('fila')}
    >
      Demandas com prazo
    </button>
    <button
      type="button"
      class={`tab ${activeTab === 'crm' ? 'is-active' : ''}`}
      onclick={() => selectTab('crm')}
    >
      Quem te procurou
    </button>
    <button
      type="button"
      class={`tab ${activeTab === 'compromissos' ? 'is-active' : ''}`}
      onclick={() => selectTab('compromissos')}
    >
      Compromissos
    </button>
    <button
      type="button"
      class={`tab ${activeTab === 'op' ? 'is-active' : ''}`}
      onclick={() => selectTab('op')}
    >
      Orçamento participativo
    </button>
  </nav>

  {#if activeTab === 'fila'}
  <section class="counters" aria-label="Resumo">
    <div class="counter">
      <strong>{pending.length}</strong>
      <span>com prazo correndo</span>
    </div>
    <div class="counter ok">
      <strong>{settled.filter((s) => s.status === 'answered').length}</strong>
      <span>respondidas</span>
    </div>
    <div class="counter bad">
      <strong>{settled.filter((s) => s.status === 'ignored').length}</strong>
      <span>silêncio registrado</span>
    </div>
  </section>

  <section>
    <h2>
      ⏰ Demandas com prazo correndo ({pending.length})
    </h2>
    {#if pending.length === 0}
      <p class="muted">Nenhuma demanda em prazo aberto neste momento.</p>
    {:else}
      <ul class="sla-list">
        {#each pending as s (s.id)}
          {@const p = proposalsById.get(s.proposal_id)}
          <li class="sla-card">
            <div class="title-row">
              <a class="prop-link" href={`/propostas/${s.proposal_id}`}>
                <strong>{p?.title ?? 'Proposta'}</strong>
              </a>
              <button
                class="btn btn-primary btn-sm"
                type="button"
                onclick={() => open(s.id)}
              >
                {openId === s.id ? 'Cancelar' : 'Responder'}
              </button>
            </div>
            <SlaClock
              dueAt={s.due_at}
              startedAt={s.started_at}
              status={s.status as SlaStatus}
            />
            {#if openId === s.id}
              <form class="respond-form" onsubmit={(e) => { e.preventDefault(); submit(s.id); }}>
                <label for={`b-${s.id}`}>Sua resposta pública</label>
                <textarea
                  id={`b-${s.id}`}
                  class="input"
                  rows="5"
                  bind:value={body}
                  placeholder="Explique como vai agir sobre esta demanda."
                  maxlength="4000"
                ></textarea>
                <label class="commit">
                  <input type="checkbox" bind:checked={committed} />
                  Estou assumindo um <strong>compromisso público</strong> de ação
                  sobre esta demanda (e não só respondendo).
                </label>
                <button
                  class="btn btn-primary"
                  type="submit"
                  disabled={busy || body.trim().length < 10}
                >
                  {busy ? 'Enviando…' : 'Publicar resposta'}
                </button>
                {#if status}
                  <p class={`hint ${status.kind === 'ok' ? 'hint-ok' : 'hint-error'}`}>
                    {status.text}
                  </p>
                {/if}
              </form>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  </section>

  {#if settled.length > 0}
    <section>
      <h2>Histórico de respostas ({settled.length})</h2>
      <ul class="sla-list compact">
        {#each settled as s (s.id)}
          {@const p = proposalsById.get(s.proposal_id)}
          <li class="sla-card-compact">
            <a class="prop-link" href={`/propostas/${s.proposal_id}`}>
              {p?.title ?? 'Proposta'}
            </a>
            <span class={`badge badge-${s.status}`}>
              {s.status === 'answered'
                ? '✓ Respondida'
                : s.status === 'acted'
                  ? '✓ Com compromisso'
                  : '✗ Silêncio público'}
            </span>
          </li>
        {/each}
      </ul>
    </section>
  {/if}
  {/if}

  {#if activeTab === 'crm'}
    <section class="crm">
      <p class="crm-intro muted">
        Quem procurou o seu mandato e o que pediu — uma ferramenta de
        <strong>relacionamento</strong> com a sua base. Só mostra o que já é
        público (autoria de propostas dirigidas a você); nunca votos ou apoios
        privados.
      </p>

      {#if crmLoading && !crm}
        <p class="muted">Carregando…</p>
      {:else if crmError}
        <p class="hint hint-error" role="alert">{crmError}</p>
      {:else if crm}
        <div class="counters" aria-label="Resumo do CRM">
          <div class="counter">
            <strong>{crm.totals.contacts}</strong>
            <span>pessoas</span>
          </div>
          <div class="counter">
            <strong>{crm.totals.demands}</strong>
            <span>demandas</span>
          </div>
          <div class="counter ok">
            <strong>{crm.totals.answered}</strong>
            <span>respondidas</span>
          </div>
          <div class="counter bad">
            <strong>{crm.totals.silence}</strong>
            <span>silêncio</span>
          </div>
        </div>

        <div class="crm-filters">
          <label class="crm-filter">
            <span>Status</span>
            <select
              class="input"
              bind:value={crmStatus}
              onchange={() => loadCrm()}
            >
              <option value="">Todos</option>
              <option value="respondida">Respondidas</option>
              <option value="pendente">Prazo correndo</option>
              <option value="silencio">Silêncio</option>
              <option value="aberta">Reunindo apoios</option>
            </select>
          </label>

          {#if crm.themes.length > 0}
            <div class="themes" role="group" aria-label="Filtrar por tema">
              {#each crm.themes as t (t.theme)}
                <button
                  type="button"
                  class={`chip ${crmTheme === t.theme ? 'is-active' : ''}`}
                  onclick={() => toggleTheme(t.theme)}
                >
                  {t.theme} ({t.demands_count})
                </button>
              {/each}
            </div>
          {/if}
        </div>

        {#if crm.contacts.length === 0}
          <p class="muted">
            Ninguém dirigiu uma proposta pública a este mandato ainda
            {crmStatus || crmTheme ? ' com esse filtro' : ''}.
          </p>
        {:else}
          <ul class="crm-list">
            {#each crm.contacts as c (c.citizen_id)}
              <li class="crm-card">
                <button
                  type="button"
                  class="crm-person"
                  onclick={() => toggleContact(c.citizen_id)}
                  aria-expanded={openContacts.has(c.citizen_id)}
                >
                  {#if c.avatar_url}
                    <img class="crm-avatar" src={c.avatar_url} alt="" />
                  {:else}
                    <span class="crm-avatar crm-avatar-ph">👤</span>
                  {/if}
                  <span class="crm-who">
                    <strong>{contactName(c)}</strong>
                    <span class="muted small">
                      @{c.handle ?? c.public_handle} ·
                      {c.demands_count} demanda{c.demands_count === 1 ? '' : 's'} ·
                      último contato {fmtDate(c.last_contact_at)}
                    </span>
                  </span>
                  <span class="crm-mini">
                    {#if c.answered_count > 0}<span class="badge badge-answered">{c.answered_count} resp.</span>{/if}
                    {#if c.pending_count > 0}<span class="badge badge-pending">{c.pending_count} prazo</span>{/if}
                    {#if c.silence_count > 0}<span class="badge badge-ignored">{c.silence_count} silêncio</span>{/if}
                    {#if c.open_count > 0}<span class="badge badge-open">{c.open_count} aberta</span>{/if}
                  </span>
                </button>

                {#if openContacts.has(c.citizen_id)}
                  <table class="crm-demands">
                    <thead>
                      <tr>
                        <th>Demanda</th>
                        <th>Tema</th>
                        <th>Status</th>
                        <th>Data</th>
                      </tr>
                    </thead>
                    <tbody>
                      {#each c.demands as d (d.proposal_id)}
                        <tr>
                          <td>
                            <a class="prop-link" href={`/propostas/${d.proposal_id}`}>
                              {d.title}
                            </a>
                            {#if d.urgencia === 'urgente'}
                              <span class="urgente">🔥</span>
                            {/if}
                          </td>
                          <td>{d.theme}</td>
                          <td><span class={`badge badge-${d.status}`}>{CRM_STATUS_LABEL[d.status]}</span></td>
                          <td class="nowrap">{fmtDate(d.created_at)}</td>
                        </tr>
                      {/each}
                    </tbody>
                  </table>
                {/if}
              </li>
            {/each}
          </ul>
        {/if}
      {/if}
    </section>
  {/if}

  {#if activeTab === 'compromissos'}
    <section class="commit">
      <p class="crm-intro muted">
        Um <strong>compromisso consultivo voluntário</strong>: o mandato declara
        que vai <strong>ouvir a base</strong> antes de decidir sobre um tema — e
        publica depois se seguiu ou não. Não é vinculante (mandato é indelegável
        por lei); é <strong>transparência do compromisso</strong>, não coerção.
      </p>

      <form
        class="commit-form"
        onsubmit={(e) => {
          e.preventDefault();
          submitCommitment();
        }}
      >
        <label for="c-theme">Tema</label>
        <input
          id="c-theme"
          class="input"
          type="text"
          bind:value={newTheme}
          maxlength="120"
          placeholder="Ex.: Voto no Plano Diretor"
        />
        <label for="c-desc">O que você se compromete a fazer</label>
        <textarea
          id="c-desc"
          class="input"
          rows="3"
          bind:value={newDescription}
          maxlength="2000"
          placeholder="Ex.: Vou consultar a base antes de votar e seguir o que a maioria orientar."
        ></textarea>
        <button
          class="btn btn-primary"
          type="submit"
          disabled={creating || !newTheme.trim() || !newDescription.trim()}
        >
          {creating ? 'Declarando…' : 'Declarar compromisso'}
        </button>
      </form>

      {#if commitMsg}
        <p class={`hint ${commitMsg.kind === 'ok' ? 'hint-ok' : 'hint-error'}`} role="status">
          {commitMsg.text}
        </p>
      {/if}

      {#if commitLoading && commitments.length === 0}
        <p class="muted">Carregando…</p>
      {:else if commitError}
        <p class="hint hint-error" role="alert">{commitError}</p>
      {:else if commitments.length === 0}
        <p class="muted">Nenhum compromisso declarado ainda.</p>
      {:else}
        <ul class="commit-list">
          {#each commitments as c (c.id)}
            <li class="commit-card">
              <div class="commit-head">
                <strong class="commit-theme">{c.theme}</strong>
                <span class={`badge badge-outcome-${c.outcome}`}>
                  {OUTCOME_LABEL[c.outcome]}
                </span>
              </div>
              <p class="commit-desc">{c.description}</p>

              {#if c.consultation}
                <div class="commit-consulta">
                  <a
                    class="prop-link"
                    href={`/consulta/?id=${c.consultation.consultation_id}`}
                  >
                    📋 {c.consultation.title}
                  </a>
                  <span class="muted small">
                    {c.consultation.total} resposta{c.consultation.total === 1 ? '' : 's'}
                    · 👍 {c.consultation.concordo}
                    · 😐 {c.consultation.neutro}
                    · 👎 {c.consultation.discordo}
                  </span>
                </div>
              {/if}

              <div class="commit-actions">
                {#if !c.consultation}
                  <button
                    class="btn btn-ghost btn-sm"
                    type="button"
                    disabled={busyCommit === c.id}
                    onclick={() => consult(c.id)}
                  >
                    Abrir consulta à base
                  </button>
                {/if}
                {#if c.outcome === 'pendente'}
                  <button
                    class="btn btn-ghost btn-sm"
                    type="button"
                    disabled={busyCommit === c.id}
                    onclick={() => setOutcome(c.id, 'seguiu')}
                  >
                    🟢 Registrar que seguiu
                  </button>
                  <button
                    class="btn btn-ghost btn-sm"
                    type="button"
                    disabled={busyCommit === c.id}
                    onclick={() => setOutcome(c.id, 'nao_seguiu')}
                  >
                    🔴 Registrar que não seguiu
                  </button>
                {/if}
              </div>

              {#if c.outcome_note}
                <p class="commit-note muted">{c.outcome_note}</p>
              {/if}
              <p class="commit-date muted small">
                Declarado em {fmtDate(c.created_at)}
              </p>
            </li>
          {/each}
        </ul>
      {/if}
    </section>
  {/if}

  {#if activeTab === 'op'}
    <MandateOpPanel mandateId={mine.mandate.id} />
  {/if}
{/if}

<style>
  .head {
    display: flex;
    gap: 1.25rem;
    align-items: center;
    margin-bottom: 2rem;
    padding-bottom: 1.5rem;
    border-bottom: 1px solid var(--c-border);
  }
  .head h1 {
    margin: 0 0 0.4rem;
    font-size: 1.5rem;
  }
  .head .muted {
    margin: 0;
  }
  .head .small {
    font-size: 0.85rem;
  }
  .avatar {
    width: 80px;
    height: 80px;
    border-radius: 50%;
    object-fit: cover;
    background: var(--c-bg);
    flex-shrink: 0;
  }
  .avatar-placeholder {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    font-size: 2rem;
  }
  section {
    margin-bottom: 2.5rem;
  }
  section h2 {
    font-size: 1.05rem;
    margin: 0 0 1rem;
  }
  .who {
    flex: 1;
  }
  .placar {
    margin-left: auto;
    align-self: center;
    white-space: nowrap;
  }
  .counters {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 0.5rem;
    margin: 0 0 1.5rem;
  }
  @media (max-width: 480px) {
    .counters {
      grid-template-columns: 1fr;
    }
  }
  .counter {
    padding: 0.85rem 1rem;
    background: var(--c-bg, #f2f4f7);
    border: 1px solid var(--c-border);
    border-radius: 10px;
    text-align: center;
  }
  .counter strong {
    display: block;
    font-size: 1.75rem;
    line-height: 1;
    font-variant-numeric: tabular-nums;
  }
  .counter span {
    display: block;
    font-size: 0.85rem;
    color: var(--c-text-muted);
    margin-top: 4px;
  }
  .counter.ok {
    background: var(--c-green-soft, #e6f7ed);
    border-color: #b7e4c7;
  }
  .counter.ok strong {
    color: #115c2d;
  }
  .counter.bad {
    background: #fff1f0;
    border-color: #ffa39e;
  }
  .counter.bad strong {
    color: #a8071a;
  }
  .sla-list {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: 1rem;
  }
  .sla-card {
    background: var(--c-paper);
    border: 1px solid var(--c-border);
    border-radius: 12px;
    padding: 1rem;
  }
  .title-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 1rem;
    margin-bottom: 0.85rem;
    flex-wrap: wrap;
  }
  .prop-link {
    color: var(--c-navy);
    text-decoration: none;
  }
  .prop-link:hover {
    text-decoration: underline;
  }
  .btn-sm {
    padding: 0.4rem 0.85rem;
    font-size: 0.9rem;
  }
  .respond-form {
    margin-top: 1rem;
    padding-top: 1rem;
    border-top: 1px solid var(--c-border);
    display: grid;
    gap: 0.6rem;
  }
  .respond-form label {
    font-weight: 600;
    font-size: 0.95rem;
  }
  .respond-form textarea {
    width: 100%;
    resize: vertical;
    font-family: inherit;
  }
  .commit {
    display: flex;
    gap: 0.5rem;
    align-items: flex-start;
    font-weight: 400;
    font-size: 0.92rem;
    color: var(--c-text);
  }
  .commit input {
    margin-top: 0.25rem;
  }
  .sla-card-compact {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.7rem 1rem;
    background: var(--c-paper);
    border: 1px solid var(--c-border);
    border-radius: 10px;
    gap: 1rem;
    flex-wrap: wrap;
  }
  .badge-answered   { background: var(--c-green-soft); color: var(--c-green-dark); }
  .badge-acted      { background: var(--c-green-soft); color: var(--c-green-dark); font-weight: 700; }
  .badge-ignored,
  .badge-silencio   { background: #fef2f2; color: #b91c1c; }
  .badge-respondida { background: var(--c-green-soft); color: var(--c-green-dark); }
  .badge-pendente,
  .badge-pending    { background: #fff7e6; color: #ad6800; }
  .badge-aberta,
  .badge-open       { background: var(--c-bg, #f2f4f7); color: var(--c-text-muted); }
  .center {
    text-align: center;
    padding: 2.5rem 1.5rem;
  }
  .hint-ok { color: var(--c-green-dark); }

  /* --- Abas + CRM (C6) ------------------------------------------------------ */
  .tabs {
    display: flex;
    gap: 0.25rem;
    margin: 0 0 1.5rem;
    border-bottom: 1px solid var(--c-border);
  }
  .tab {
    appearance: none;
    background: none;
    border: none;
    border-bottom: 2px solid transparent;
    padding: 0.6rem 1rem;
    font: inherit;
    font-weight: 600;
    color: var(--c-text-muted);
    cursor: pointer;
  }
  .tab.is-active {
    color: var(--c-navy);
    border-bottom-color: var(--c-navy);
  }
  .crm-intro {
    margin: 0 0 1.25rem;
    font-size: 0.95rem;
  }
  .crm-filters {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-end;
    gap: 0.75rem 1.25rem;
    margin: 0 0 1.25rem;
  }
  .crm-filter {
    display: grid;
    gap: 0.25rem;
    font-size: 0.85rem;
    font-weight: 600;
  }
  .themes {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
  }
  .chip {
    appearance: none;
    border: 1px solid var(--c-border);
    background: var(--c-paper);
    border-radius: 999px;
    padding: 0.3rem 0.75rem;
    font: inherit;
    font-size: 0.82rem;
    cursor: pointer;
  }
  .chip.is-active {
    background: var(--c-navy);
    color: #fff;
    border-color: var(--c-navy);
  }
  .crm-list {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: 0.75rem;
  }
  .crm-card {
    background: var(--c-paper);
    border: 1px solid var(--c-border);
    border-radius: 12px;
    overflow: hidden;
  }
  .crm-person {
    display: flex;
    align-items: center;
    gap: 0.85rem;
    width: 100%;
    padding: 0.85rem 1rem;
    background: none;
    border: none;
    font: inherit;
    text-align: left;
    cursor: pointer;
    flex-wrap: wrap;
  }
  .crm-avatar {
    width: 40px;
    height: 40px;
    border-radius: 50%;
    object-fit: cover;
    flex-shrink: 0;
    background: var(--c-bg);
  }
  .crm-avatar-ph {
    display: inline-flex;
    align-items: center;
    justify-content: center;
  }
  .crm-who {
    display: grid;
    gap: 2px;
    flex: 1;
    min-width: 12rem;
  }
  .crm-mini {
    display: flex;
    gap: 0.35rem;
    flex-wrap: wrap;
    margin-left: auto;
  }
  .badge {
    display: inline-block;
    padding: 0.15rem 0.5rem;
    border-radius: 999px;
    font-size: 0.78rem;
    white-space: nowrap;
  }
  .crm-demands {
    width: 100%;
    border-collapse: collapse;
    border-top: 1px solid var(--c-border);
    font-size: 0.9rem;
  }
  .crm-demands th,
  .crm-demands td {
    text-align: left;
    padding: 0.5rem 1rem;
    border-bottom: 1px solid var(--c-border);
    vertical-align: top;
  }
  .crm-demands th {
    font-size: 0.78rem;
    text-transform: uppercase;
    letter-spacing: 0.03em;
    color: var(--c-text-muted);
  }
  .crm-demands tr:last-child td {
    border-bottom: none;
  }
  .nowrap { white-space: nowrap; }
  .urgente { margin-left: 0.35rem; }

  /* --- Compromissos (D8.1) -------------------------------------------------- */
  .commit-form {
    display: grid;
    gap: 0.5rem;
    margin: 0 0 1.25rem;
    padding: 1rem;
    background: var(--c-paper);
    border: 1px solid var(--c-border);
    border-radius: 12px;
  }
  .commit-form label {
    font-weight: 600;
    font-size: 0.9rem;
  }
  .commit-form textarea {
    width: 100%;
    resize: vertical;
    font-family: inherit;
  }
  .commit-form .btn {
    justify-self: start;
  }
  .commit-list {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: 0.75rem;
  }
  .commit-card {
    background: var(--c-paper);
    border: 1px solid var(--c-border);
    border-radius: 12px;
    padding: 1rem;
    display: grid;
    gap: 0.6rem;
  }
  .commit-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    flex-wrap: wrap;
  }
  .commit-theme {
    font-size: 1.05rem;
  }
  .commit-desc {
    margin: 0;
    white-space: pre-wrap;
  }
  .commit-consulta {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    padding: 0.6rem 0.75rem;
    background: var(--c-bg, #f2f4f7);
    border-radius: 8px;
  }
  .commit-actions {
    display: flex;
    gap: 0.5rem;
    flex-wrap: wrap;
  }
  .commit-note {
    margin: 0;
    padding-left: 0.75rem;
    border-left: 3px solid var(--c-border);
    font-style: italic;
  }
  .commit-date {
    margin: 0;
  }
  .badge-outcome-seguiu {
    background: var(--c-green-soft, #e6f7ed);
    color: var(--c-green-dark, #115c2d);
  }
  .badge-outcome-nao_seguiu {
    background: #fef2f2;
    color: #b91c1c;
  }
  .badge-outcome-pendente {
    background: #fff7e6;
    color: #ad6800;
  }
  @media (max-width: 560px) {
    .crm-demands thead { display: none; }
    .crm-demands,
    .crm-demands tbody,
    .crm-demands tr,
    .crm-demands td { display: block; width: 100%; }
    .crm-demands td { border-bottom: none; padding: 0.2rem 1rem; }
    .crm-demands tr { border-bottom: 1px solid var(--c-border); padding: 0.5rem 0; }
  }
</style>
