<script lang="ts">
  // GUI completa de usuários — /admin/usuarios.
  //
  // Layout: filtros no topo (busca + selects), tabela abaixo com paginação
  // via "Carregar mais". Cada linha tem colunas para papel plataforma,
  // papel partido, filiação, verificação, público. Edição via drawer
  // lateral (evita modal-hell) — clique em "Editar" abre com todos os
  // campos.
  //
  // Filtros e paginação são via query strings — refresh preserva estado.
  import { onMount } from 'svelte';
  import {
    listAdminUsers,
    patchAdminUser,
    setPlatformRole,
    setPartyRole,
    adminSuspendAccount,
    adminUnsuspendAccount,
    adminSilenceAccount,
    adminUnsilenceAccount,
    type AdminUserRow,
  } from '../../lib/api';
  import { toast } from '../../lib/toasts';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';
  import Input from '../ui/Input.svelte';
  import Badge from '../ui/Badge.svelte';
  import Alert from '../ui/Alert.svelte';
  import EmptyState from '../ui/EmptyState.svelte';
  import Spinner from '../ui/Spinner.svelte';

  const PAGE = 50;

  // Lista de siglas de partido — hardcoded pra evitar 1 endpoint a mais.
  // TSE tem ~30 partidos ativos; se algum admin usar sigla nova, a UI deixa
  // digitar livremente no drawer.
  const PARTIES = [
    'AVANTE', 'CIDADANIA', 'DC', 'MDB', 'MOBILIZA', 'NOVO', 'PC do B',
    'PDT', 'PL', 'PMB', 'PODE', 'PP', 'PRD', 'PRTB', 'PSB', 'PSD',
    'PSDB', 'PSOL', 'PSTU', 'PT', 'PV', 'REDE', 'REPUBLICANOS',
    'SOLIDARIEDADE', 'UNIÃO', 'UP',
  ];

  let rows = $state<AdminUserRow[]>([]);
  let loading = $state(true);
  let loadErr = $state<string | null>(null);
  let hasMore = $state(false);

  // Filters
  let q = $state('');
  let party = $state('');
  let platformRole = $state<'any' | 'owner' | 'admin' | 'auditor' | 'none'>(
    'any',
  );
  let partyRole = $state<'any' | 'admin' | 'moderador' | 'none'>('any');
  let civicType = $state<'any' | 'cidadao' | 'politico' | 'candidato'>('any');
  let offset = $state(0);

  // Drawer state
  let editing = $state<AdminUserRow | null>(null);
  let draftParty = $state('');
  let draftPlatformRole = $state<'none' | 'owner' | 'admin' | 'auditor'>('none');
  let draftPartyAdminRole = $state<'none' | 'admin' | 'moderador'>('none');
  let draftPartyAdminSigla = $state('');
  let draftIsPublic = $state(true);
  let draftVerif = $state('email');
  let saving = $state(false);
  let saveMsg = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);
  // Ações de moderação em voo (por citizen_id).
  let modBusy = $state<Set<string>>(new Set());

  async function runMod(action: 'suspend' | 'unsuspend' | 'silence' | 'unsilence') {
    if (!editing) return;
    const id = editing.citizen_id;
    if (modBusy.has(id)) return;
    modBusy = new Set(modBusy).add(id);
    let res;
    if (action === 'suspend') {
      const reason = prompt('Razão da suspensão (opcional, aparece no audit log):') ?? undefined;
      res = await adminSuspendAccount(id, reason || undefined);
    } else if (action === 'unsuspend') {
      res = await adminUnsuspendAccount(id);
    } else if (action === 'silence') {
      const reason = prompt('Razão do silenciamento (opcional, aparece no audit log):') ?? undefined;
      res = await adminSilenceAccount(id, reason || undefined);
    } else {
      res = await adminUnsilenceAccount(id);
    }
    const done = new Set(modBusy);
    done.delete(id);
    modBusy = done;
    if (res.success) {
      toast.success(
        action === 'suspend' ? 'Conta suspensa.'
        : action === 'unsuspend' ? 'Suspensão removida.'
        : action === 'silence' ? 'Conta silenciada.'
        : 'Silenciamento removido.',
      );
      // Refresca a lista e sincroniza o editing.
      await refresh();
      const fresh = rows.find((r) => r.citizen_id === id);
      if (fresh) editing = fresh;
    } else {
      toast.error(res.error?.message ?? 'Falha na ação de moderação.');
    }
  }

  async function refresh(append = false) {
    loading = true;
    if (!append) offset = 0;
    const res = await listAdminUsers({
      q: q || undefined,
      party: party || undefined,
      platform_role: platformRole,
      party_role: partyRole,
      civic_type: civicType,
      limit: PAGE,
      offset,
    });
    loading = false;
    if (res.success && res.data) {
      rows = append ? [...rows, ...res.data] : res.data;
      hasMore = res.data.length === PAGE;
      loadErr = null;
    } else {
      loadErr =
        res.error?.message ??
        'Não foi possível listar. Você tem papel admin/owner nesta instância?';
    }
  }

  function edit(u: AdminUserRow) {
    editing = u;
    draftParty = u.party_sigla ?? '';
    draftPlatformRole = (u.platform_role as any) ?? 'none';
    draftPartyAdminRole = (u.party_admin_role as any) ?? 'none';
    draftPartyAdminSigla = u.party_admin_sigla ?? u.party_sigla ?? '';
    draftIsPublic = u.is_public;
    draftVerif = u.verification_level;
    saveMsg = null;
  }

  function closeDrawer() {
    editing = null;
    saveMsg = null;
  }

  async function saveAll() {
    if (!editing || saving) return;
    saving = true;
    saveMsg = null;

    // 1. Campos do citizen (party_sigla, verification, is_public)
    const partyForCitizen: string | null =
      draftParty.trim() === '' ? null : draftParty.trim().toUpperCase();
    const patchRes = await patchAdminUser(editing.citizen_id, {
      party_sigla: partyForCitizen,
      verification_level: draftVerif,
      is_public: draftIsPublic,
    });
    if (!patchRes.success) {
      saving = false;
      saveMsg = { kind: 'error', text: patchRes.error?.message ?? 'Falha.' };
      return;
    }

    // 2. Papel plataforma
    const plr = await setPlatformRole(editing.citizen_id, draftPlatformRole);
    if (!plr.success) {
      saving = false;
      saveMsg = { kind: 'error', text: plr.error?.message ?? 'Falha (plataforma).' };
      return;
    }

    // 3. Papel partido
    const partyForRole =
      draftPartyAdminSigla.trim() || partyForCitizen || '';
    const par = await setPartyRole(
      editing.citizen_id,
      draftPartyAdminRole,
      draftPartyAdminRole === 'none' ? undefined : partyForRole.toUpperCase(),
    );
    if (!par.success) {
      saving = false;
      saveMsg = { kind: 'error', text: par.error?.message ?? 'Falha (partido).' };
      return;
    }

    saving = false;
    saveMsg = { kind: 'ok', text: 'Salvo.' };
    await refresh();
    // Fecha o drawer depois de 800ms pra o admin ver o "Salvo".
    window.setTimeout(closeDrawer, 800);
  }

  function fmtDate(iso: string) {
    return new Date(iso).toLocaleDateString('pt-BR', {
      year: '2-digit',
      month: '2-digit',
      day: '2-digit',
    });
  }

  function civicBadges(u: AdminUserRow) {
    const out: { label: string; tone: 'success' | 'accent' | 'neutral' }[] = [];
    if (u.has_candidacy) out.push({ label: 'candidatura', tone: 'accent' });
    else if (u.has_mandate) out.push({ label: 'político', tone: 'success' });
    else out.push({ label: 'cidadão', tone: 'neutral' });
    return out;
  }

  onMount(() => {
    void refresh();
  });
</script>

<Card>
  <header class="filters" aria-label="Filtros">
    <div class="q">
      <Input
        id="admin-users-q"
        label=""
        placeholder="Busca por nome, @handle ou e-mail…"
        bind:value={q}
        onkeydown={(e: KeyboardEvent) => e.key === 'Enter' && refresh()}
      />
    </div>
    <div class="sel">
      <label>
        <span>Partido (filiação)</span>
        <select bind:value={party} onchange={() => refresh()}>
          <option value="">Todos</option>
          {#each PARTIES as p}
            <option value={p}>{p}</option>
          {/each}
        </select>
      </label>
      <label>
        <span>Papel plataforma</span>
        <select bind:value={platformRole} onchange={() => refresh()}>
          <option value="any">Todos</option>
          <option value="owner">Owner</option>
          <option value="admin">Admin</option>
          <option value="auditor">Auditor</option>
          <option value="none">Nenhum</option>
        </select>
      </label>
      <label>
        <span>Papel partido</span>
        <select bind:value={partyRole} onchange={() => refresh()}>
          <option value="any">Todos</option>
          <option value="admin">Admin de partido</option>
          <option value="moderador">Moderador</option>
          <option value="none">Nenhum</option>
        </select>
      </label>
      <label>
        <span>Tipo cívico</span>
        <select bind:value={civicType} onchange={() => refresh()}>
          <option value="any">Todos</option>
          <option value="cidadao">Cidadão comum</option>
          <option value="politico">Político (mandato)</option>
          <option value="candidato">Candidato</option>
        </select>
      </label>
    </div>
    <div class="acts">
      <Button variant="secondary" onclick={() => refresh()}>Aplicar</Button>
    </div>
  </header>

  {#if loading && rows.length === 0}
    <div class="center"><Spinner /></div>
  {:else if loadErr}
    <Alert tone="danger">{loadErr}</Alert>
  {:else if rows.length === 0}
    <EmptyState
      icon="users"
      title="Nenhum cidadão bate os filtros"
      description="Ajuste os filtros ou limpe a busca."
    />
  {:else}
    <div class="tbl-wrap">
      <table class="tbl">
        <thead>
          <tr>
            <th>Cidadão</th>
            <th>E-mail</th>
            <th>Perfil</th>
            <th>Filiação</th>
            <th>Papel</th>
            <th>Partido admin</th>
            <th>Verif.</th>
            <th class="right">Criado</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          {#each rows as u (u.citizen_id)}
            <tr>
              <td>
                <div class="who">
                  <strong>
                    {u.display_name || u.handle || u.citizen_id.slice(0, 8)}
                  </strong>
                  {#if u.handle}
                    <span class="muted small">@{u.handle}</span>
                  {/if}
                </div>
              </td>
              <td class="mono small">{u.email ?? '—'}</td>
              <td>
                <div class="chips">
                  {#each civicBadges(u) as b}
                    <Badge tone={b.tone} size="sm">{b.label}</Badge>
                  {/each}
                  {#if u.titulo_status === 'validated' || u.titulo_status === 'verified'}
                    <Badge tone="success" size="sm">título ✓</Badge>
                  {/if}
                  {#if u.cpf_status === 'validated'}
                    <Badge tone="success" size="sm">CPF ✓</Badge>
                  {:else if u.cpf_status}
                    <Badge tone="warning" size="sm">CPF: {u.cpf_status}</Badge>
                  {/if}
                  {#if !u.is_public}
                    <Badge tone="neutral" size="sm">privado</Badge>
                  {/if}
                </div>
              </td>
              <td>
                {#if u.party_sigla}
                  <Badge tone="accent" size="sm">{u.party_sigla}</Badge>
                {:else}
                  <span class="muted small">—</span>
                {/if}
              </td>
              <td>
                {#if u.platform_role}
                  <Badge tone="warning" size="sm">{u.platform_role}</Badge>
                {:else}
                  <span class="muted small">—</span>
                {/if}
              </td>
              <td>
                {#if u.party_admin_role}
                  <span class="mono small">
                    {u.party_admin_sigla}/{u.party_admin_role}
                  </span>
                {:else}
                  <span class="muted small">—</span>
                {/if}
              </td>
              <td class="small">{u.verification_level}</td>
              <td class="right muted small">{fmtDate(u.created_at)}</td>
              <td>
                <Button variant="ghost" size="sm" onclick={() => edit(u)}>
                  Editar
                </Button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
    {#if hasMore}
      <div class="foot">
        <Button
          variant="ghost"
          onclick={() => {
            offset += PAGE;
            void refresh(true);
          }}
          loading={loading}
        >
          Carregar mais
        </Button>
      </div>
    {/if}
  {/if}
</Card>

<!-- Drawer de edição -->
{#if editing}
  <button
    type="button"
    class="scrim"
    aria-label="Fechar"
    onclick={closeDrawer}
  ></button>
  <aside class="drawer" aria-label="Editar cidadão">
    <header>
      <h3>
        {editing.display_name || editing.handle || editing.citizen_id.slice(0, 8)}
      </h3>
      <button
        type="button"
        class="close"
        aria-label="Fechar"
        onclick={closeDrawer}
      >
        ✕
      </button>
    </header>
    <p class="muted small">
      <span class="mono">{editing.citizen_id}</span>
      {#if editing.email}
        <br />
        <span class="mono">{editing.email}</span>
      {/if}
    </p>

    <div class="field">
      <label for="d-party">Filiação partidária</label>
      <select id="d-party" bind:value={draftParty}>
        <option value="">— nenhum —</option>
        {#each PARTIES as p}
          <option value={p}>{p}</option>
        {/each}
      </select>
      <span class="hint muted small">
        Só informativo — não altera permissões.
      </span>
    </div>

    <div class="field">
      <label for="d-verif">Nível de verificação</label>
      <select id="d-verif" bind:value={draftVerif}>
        <option value="anonymous">anonymous</option>
        <option value="email">email</option>
        <option value="directory">directory</option>
        <option value="strong">strong</option>
      </select>
    </div>

    <div class="field row">
      <label class="check">
        <input type="checkbox" bind:checked={draftIsPublic} />
        <span>Perfil público (aparece em busca e no fediverso)</span>
      </label>
    </div>

    <hr />

    <div class="field">
      <label for="d-plr">Papel na plataforma</label>
      <select id="d-plr" bind:value={draftPlatformRole}>
        <option value="none">— nenhum —</option>
        <option value="auditor">Auditor (read-only)</option>
        <option value="admin">Admin</option>
        <option value="owner">Owner</option>
      </select>
    </div>

    <div class="field">
      <label for="d-pa">Papel de partido</label>
      <select id="d-pa" bind:value={draftPartyAdminRole}>
        <option value="none">— nenhum —</option>
        <option value="admin">Admin</option>
        <option value="moderador">Moderador</option>
      </select>
      {#if draftPartyAdminRole !== 'none'}
        <select bind:value={draftPartyAdminSigla}>
          <option value="">Escolha o partido…</option>
          {#each PARTIES as p}
            <option value={p}>{p}</option>
          {/each}
        </select>
      {/if}
    </div>

    <hr />
    <div class="field">
      <label>Moderação da conta</label>
      <div class="mod-status">
        {#if editing.suspended_at}
          <Badge tone="danger" size="sm">Suspensa</Badge>
        {/if}
        {#if editing.silenced_at}
          <Badge tone="warning" size="sm">Silenciada</Badge>
        {/if}
        {#if !editing.suspended_at && !editing.silenced_at}
          <span class="muted small">Sem restrições.</span>
        {/if}
      </div>
      <div class="mod-actions">
        {#if editing.suspended_at}
          <Button variant="ghost" size="sm" onclick={() => runMod('unsuspend')} loading={modBusy.has(editing.citizen_id)}>
            Retirar suspensão
          </Button>
        {:else}
          <Button variant="danger" size="sm" onclick={() => runMod('suspend')} loading={modBusy.has(editing.citizen_id)}>
            Suspender
          </Button>
        {/if}
        {#if editing.silenced_at}
          <Button variant="ghost" size="sm" onclick={() => runMod('unsilence')} loading={modBusy.has(editing.citizen_id)}>
            Retirar silenciamento
          </Button>
        {:else}
          <Button variant="secondary" size="sm" onclick={() => runMod('silence')} loading={modBusy.has(editing.citizen_id)}>
            Silenciar
          </Button>
        {/if}
      </div>
      <p class="hint muted small">
        <strong>Suspender</strong>: bloqueia login, oculta a conta e seu conteúdo, encerra sessões ativas.
        <strong>Silenciar</strong>: notas só aparecem para quem já segue; conta some do diretório público.
      </p>
    </div>

    <footer>
      {#if saveMsg}
        <div class="alert-slot">
          <Alert tone={saveMsg.kind === 'ok' ? 'success' : 'danger'}>
            {saveMsg.text}
          </Alert>
        </div>
      {/if}
      <div class="row">
        <Button variant="ghost" onclick={closeDrawer}>Cancelar</Button>
        <Button variant="primary" onclick={saveAll} loading={saving}>
          Salvar
        </Button>
      </div>
    </footer>
  </aside>
{/if}

<style>
  .filters {
    display: grid;
    gap: var(--sp-3);
    margin-bottom: var(--sp-4);
    padding-bottom: var(--sp-3);
    border-bottom: 1px solid var(--border-subtle);
  }
  .q {
    max-width: 26rem;
  }
  .sel {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: var(--sp-2);
  }
  @media (max-width: 900px) {
    .sel {
      grid-template-columns: repeat(2, 1fr);
    }
  }
  .sel label {
    display: grid;
    gap: 4px;
  }
  .sel label span {
    font-size: var(--fs-xs);
    color: var(--text-3);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  select {
    padding: var(--sp-2);
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-sm);
    color: var(--text-1);
    font-family: inherit;
    font-size: var(--fs-sm);
    width: 100%;
  }
  .acts {
    display: flex;
    justify-content: flex-end;
  }
  .tbl-wrap {
    overflow-x: auto;
  }
  .tbl {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--fs-sm);
  }
  .tbl th,
  .tbl td {
    text-align: left;
    padding: var(--sp-2) var(--sp-3);
    border-bottom: 1px solid var(--border-subtle);
    vertical-align: top;
  }
  .tbl thead th {
    font-size: var(--fs-xs);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-3);
    background: var(--surface-2);
  }
  .tbl .right {
    text-align: right;
  }
  .tbl .who {
    display: grid;
    gap: 2px;
  }
  .tbl .chips {
    display: flex;
    gap: 4px;
    flex-wrap: wrap;
  }
  .mono {
    font-family: ui-monospace, monospace;
  }
  .small {
    font-size: var(--fs-xs);
  }
  .center {
    display: grid;
    place-items: center;
    padding: var(--sp-5);
  }
  .foot {
    padding-top: var(--sp-3);
    display: flex;
    justify-content: center;
  }

  /* Drawer */
  .scrim {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.4);
    border: 0;
    padding: 0;
    cursor: pointer;
    z-index: 90;
  }
  .drawer {
    position: fixed;
    top: 0;
    right: 0;
    bottom: 0;
    width: min(28rem, 90vw);
    background: var(--surface-1);
    border-left: 1px solid var(--border-subtle);
    padding: var(--sp-4);
    display: grid;
    grid-template-rows: auto auto auto 1fr auto;
    gap: var(--sp-3);
    z-index: 100;
    box-shadow: -8px 0 24px rgba(0, 0, 0, 0.15);
    overflow-y: auto;
  }
  .drawer header {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  .drawer h3 {
    margin: 0;
    font-size: var(--fs-lg);
  }
  .drawer .close {
    background: none;
    border: none;
    font-size: 20px;
    cursor: pointer;
    color: var(--text-3);
  }
  .drawer hr {
    border: 0;
    border-top: 1px dashed var(--border-subtle);
    margin: var(--sp-2) 0;
  }
  .mod-status {
    display: flex;
    gap: var(--sp-2);
    align-items: center;
    flex-wrap: wrap;
    margin-bottom: var(--sp-2);
  }
  .mod-actions {
    display: flex;
    gap: var(--sp-2);
    flex-wrap: wrap;
    margin-bottom: var(--sp-2);
  }
  .field {
    display: grid;
    gap: 6px;
  }
  .field.row {
    grid-template-columns: 1fr;
  }
  .field label {
    font-size: var(--fs-sm);
    color: var(--text-2);
    font-weight: var(--fw-semibold);
  }
  .field .hint {
    margin-top: 2px;
  }
  .check {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    font-weight: var(--fw-medium);
    color: var(--text-1);
  }
  .drawer footer {
    display: grid;
    gap: var(--sp-2);
    padding-top: var(--sp-3);
    border-top: 1px solid var(--border-subtle);
  }
  .drawer footer .row {
    display: flex;
    gap: var(--sp-2);
    justify-content: flex-end;
  }
  .alert-slot {
    margin-bottom: var(--sp-2);
  }
</style>
