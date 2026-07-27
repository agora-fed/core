<script lang="ts">
  // Tela de consentimento de campanha do cidadão (ÁGORA F2, #59). LGPD art.11
  // (dados sensíveis): opt-in específico, DEFAULT OFF, revogável. 4 níveis de
  // capilaridade. Consome /me/campaign-consent (EN, ADR-0013).
  import { onMount } from 'svelte';
  import {
    listCampaignConsent,
    grantCampaignConsent,
    revokeCampaignConsent,
    getParties,
    getMunicipios,
    type CampaignConsentDto,
    type PartyDto,
    type MunicipioIbge,
  } from '../../lib/api';
  import { UFS } from '../../lib/ufs';
  import { toast } from '../../lib/toasts';

  type Scope = 'all_parties' | 'party' | 'municipality' | 'directory';

  let loading = $state(true);
  let error = $state<string | null>(null);
  let grants = $state<CampaignConsentDto[]>([]);
  let parties = $state<PartyDto[]>([]);

  // Form
  let scope = $state<Scope>('all_parties');
  let party = $state('');
  let uf = $state('');
  let municipio = $state('');
  let municipios = $state<MunicipioIbge[]>([]);
  let munLoading = $state(false);
  let busy = $state(false);

  const SCOPE_LABEL: Record<Scope, string> = {
    all_parties: 'Todos os partidos',
    party: 'Um partido específico',
    municipality: 'Todos os partidos de um município',
    directory: 'Um diretório (um partido num município)',
  };

  async function reload() {
    loading = true;
    error = null;
    const res = await listCampaignConsent();
    loading = false;
    if (!res.success) {
      error = res.error?.message ?? 'Faça login para gerir seu consentimento.';
      return;
    }
    grants = res.data ?? [];
  }

  onMount(async () => {
    await reload();
    const p = await getParties();
    if (p.success) parties = p.data ?? [];
  });

  // Carrega municípios ao trocar a UF (para escopos com município).
  $effect(() => {
    const cur = uf;
    municipio = '';
    municipios = [];
    if (!cur || (scope !== 'municipality' && scope !== 'directory')) return;
    munLoading = true;
    getMunicipios(cur)
      .then((r) => {
        if (cur !== uf) return;
        municipios = r.success ? r.data : [];
      })
      .finally(() => (munLoading = false));
  });

  function grantLabel(g: CampaignConsentDto): string {
    switch (g.scope) {
      case 'all_parties':
        return 'Todos os partidos';
      case 'party':
        return `Partido ${g.party_sigla}`;
      case 'municipality':
        return `Todos os partidos em ${g.municipio}/${g.uf}`;
      case 'directory':
        return `${g.party_sigla} em ${g.municipio}/${g.uf}`;
      default:
        return g.scope;
    }
  }

  async function submit(e: Event) {
    e.preventDefault();
    const body: { scope: string; party_sigla?: string; uf?: string; municipio?: string } = {
      scope,
    };
    if (scope === 'party' || scope === 'directory') {
      if (!party) return toast.error('Escolha o partido');
      body.party_sigla = party;
    }
    if (scope === 'municipality' || scope === 'directory') {
      if (!uf || !municipio) return toast.error('Escolha UF e município');
      body.uf = uf;
      body.municipio = municipio;
    }
    busy = true;
    const res = await grantCampaignConsent(body);
    busy = false;
    if (!res.success) return toast.error(res.error?.message ?? 'Não foi possível salvar');
    toast.success('Consentimento registrado');
    party = '';
    uf = '';
    municipio = '';
    await reload();
  }

  async function revoke(id: string) {
    busy = true;
    const res = await revokeCampaignConsent(id);
    busy = false;
    if (!res.success) return toast.error(res.error?.message ?? 'Não foi possível revogar');
    toast.success('Consentimento revogado');
    await reload();
  }
</script>

<div class="cc">
  <p class="intro">
    Por padrão, <strong>nenhum partido ou candidato pode usar seus dados</strong> para
    comunicação de campanha. Aqui você <strong>autoriza explicitamente</strong> — e pode revogar
    a qualquer momento. Escolha o alcance: quanto mais específico, mais controle você tem.
    Opinião política é <strong>dado sensível</strong> (LGPD art. 11); nós apenas intermediamos o
    contato — sua lista nunca é entregue a ninguém.
  </p>

  {#if loading}
    <p class="muted">Carregando…</p>
  {:else if error}
    <p class="err">{error}</p>
  {:else}
    <h3>Suas autorizações</h3>
    {#if grants.length === 0}
      <p class="muted small">Você ainda não autorizou nenhum contato de campanha (padrão).</p>
    {:else}
      <ul class="rows">
        {#each grants as g (g.id)}
          <li>
            <span class="glabel">{grantLabel(g)}</span>
            <button class="rm" onclick={() => revoke(g.id)} disabled={busy}>Revogar</button>
          </li>
        {/each}
      </ul>
    {/if}

    <h3>Autorizar um novo alcance</h3>
    <form class="form" onsubmit={submit}>
      <label class="fld">
        <span>Quem pode falar comigo</span>
        <select bind:value={scope} class="input">
          <option value="all_parties">{SCOPE_LABEL.all_parties}</option>
          <option value="party">{SCOPE_LABEL.party}</option>
          <option value="municipality">{SCOPE_LABEL.municipality}</option>
          <option value="directory">{SCOPE_LABEL.directory}</option>
        </select>
      </label>

      {#if scope === 'party' || scope === 'directory'}
        <label class="fld">
          <span>Partido</span>
          <select bind:value={party} class="input" required>
            <option value="" disabled>Selecione</option>
            {#each parties as p (p.sigla)}
              <option value={p.sigla}>{p.sigla} — {p.name}</option>
            {/each}
          </select>
        </label>
      {/if}

      {#if scope === 'municipality' || scope === 'directory'}
        <label class="fld">
          <span>Estado (UF)</span>
          <select bind:value={uf} class="input" required>
            <option value="" disabled>UF</option>
            {#each UFS as u (u.code)}
              <option value={u.code}>{u.code}</option>
            {/each}
          </select>
        </label>
        <label class="fld">
          <span>Município</span>
          <select bind:value={municipio} class="input" required disabled={!uf || munLoading}>
            <option value="" disabled>
              {munLoading ? 'Carregando…' : uf ? 'Selecione' : 'Escolha a UF'}
            </option>
            {#each municipios as m (m.codigo_ibge)}
              <option value={m.nome}>{m.nome}</option>
            {/each}
          </select>
        </label>
      {/if}

      <button class="btn" disabled={busy}>Autorizar</button>
    </form>
  {/if}
</div>

<style>
  .cc { max-width: 40rem; }
  .intro { color: var(--text-2); line-height: var(--lh-relaxed); background: var(--surface-2); border-left: 3px solid var(--accent); border-radius: var(--r-sm); padding: var(--sp-3) var(--sp-4); margin: 0 0 var(--sp-4); }
  h3 { font-size: var(--fs-md); margin: var(--sp-5) 0 var(--sp-2); color: var(--text-1); }
  .rows { list-style: none; margin: 0; padding: 0; }
  .rows li { display: flex; align-items: center; gap: var(--sp-3); padding: var(--sp-2) 0; border-top: 1px solid var(--border-subtle); }
  .glabel { color: var(--text-1); font-weight: var(--fw-semibold); }
  .rm { margin-left: auto; border: 1px solid var(--border-subtle); background: var(--surface-1); border-radius: var(--r-sm); padding: 2px 10px; font-size: var(--fs-sm); cursor: pointer; color: #b91c1c; }
  .form { display: grid; gap: var(--sp-3); margin-top: var(--sp-2); }
  .fld { display: grid; gap: 4px; }
  .fld span { font-size: var(--fs-sm); font-weight: var(--fw-semibold); color: var(--text-2); }
  .input { padding: var(--sp-2) var(--sp-3); border: 1px solid var(--border-subtle); border-radius: var(--r-sm); background: var(--surface-1); color: var(--text-1); font-size: var(--fs-sm); }
  .btn { justify-self: start; padding: var(--sp-2) var(--sp-5); border-radius: var(--r-sm); border: 1px solid var(--accent); background: var(--accent); color: #fff; font-weight: var(--fw-semibold); font-size: var(--fs-sm); cursor: pointer; }
  .btn:disabled { opacity: 0.6; }
  .muted { color: var(--text-3); }
  .small { font-size: var(--fs-sm); }
  .err { color: #b91c1c; }
</style>
