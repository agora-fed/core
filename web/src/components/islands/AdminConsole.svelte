<script lang="ts">
  // Administrative console for the DemocraciaBR instance. Every request is
  // gated by the backend's `require_admin` check on DEFAULT_ORG_UUID — a
  // non-admin caller sees the "acesso restrito" empty state below.
  //
  // The panel is tabbed so a single client:load island covers all four
  // sections (Dashboard / Usuários / Federação / Moderação) without extra
  // navigation cost. The active tab is mirrored to the URL hash so a reload
  // preserves the operator's context.
  import { onMount } from 'svelte';
  import Tabs from '../ui/Tabs.svelte';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';
  import Alert from '../ui/Alert.svelte';
  import Badge from '../ui/Badge.svelte';
  import Icon from '../ui/Icon.svelte';
  import Input from '../ui/Input.svelte';
  import EmptyState from '../ui/EmptyState.svelte';
  import Spinner from '../ui/Spinner.svelte';
  import Skeleton from '../ui/Skeleton.svelte';
  import {
    getAdminStats,
    getAdminUsers,
    setAdminUserRole,
    getAdminPeers,
    hideAdminNote,
    type AdminStatsDto,
    type AdminUserRow,
    type AdminPeerRow,
  } from '../../lib/api';

  import EmailTemplatesAdmin from './EmailTemplatesAdmin.svelte';

  type TabId = 'painel' | 'usuarios' | 'federacao' | 'moderacao' | 'emails';
  const tabs: { id: TabId; label: string }[] = [
    { id: 'painel', label: 'Painel' },
    { id: 'usuarios', label: 'Usuários' },
    { id: 'federacao', label: 'Federação' },
    { id: 'moderacao', label: 'Moderação' },
    { id: 'emails', label: 'E-mails' },
  ];
  let active = $state<TabId>('painel');

  // Global gate state — once the first authenticated request comes back with
  // 401/403 we flip `denied` and stop firing further backend calls.
  let denied = $state<null | 'anon' | 'not-admin'>(null);

  // --- Dashboard state ---
  let stats = $state<AdminStatsDto | null>(null);
  let statsLoading = $state(false);
  let statsErr = $state<string | null>(null);

  async function loadStats() {
    statsLoading = true;
    statsErr = null;
    const res = await getAdminStats();
    statsLoading = false;
    if (res.success && res.data) {
      stats = res.data;
    } else if (res.error?.code === 'http_401') {
      denied = 'anon';
    } else if (res.error?.code === 'http_403') {
      denied = 'not-admin';
    } else {
      statsErr = res.error?.message ?? 'Falha ao carregar métricas.';
    }
  }

  // --- Users state ---
  let usersQ = $state('');
  let usersRows = $state<AdminUserRow[]>([]);
  let usersLoading = $state(false);
  let usersErr = $state<string | null>(null);
  let usersOffset = $state(0);
  let usersHasMore = $state(false);
  const PAGE = 25;
  let roleBusy = $state<string | null>(null);

  async function loadUsers(reset = true) {
    usersLoading = true;
    usersErr = null;
    if (reset) usersOffset = 0;
    const res = await getAdminUsers(usersQ.trim(), PAGE, usersOffset);
    usersLoading = false;
    if (res.success && res.data) {
      if (reset) usersRows = res.data;
      else usersRows = [...usersRows, ...res.data];
      usersHasMore = res.data.length === PAGE;
    } else if (res.error?.code === 'http_401') {
      denied = 'anon';
    } else if (res.error?.code === 'http_403') {
      denied = 'not-admin';
    } else {
      usersErr = res.error?.message ?? 'Falha ao carregar usuários.';
    }
  }

  async function changeRole(u: AdminUserRow, role: string | null) {
    roleBusy = u.citizen_id;
    const res = await setAdminUserRole(u.citizen_id, role as any);
    roleBusy = null;
    if (res.success && res.data) {
      usersRows = usersRows.map((row) =>
        row.citizen_id === u.citizen_id ? { ...row, role: res.data!.role } : row,
      );
    } else {
      alert(res.error?.message ?? 'Não foi possível atualizar o papel.');
    }
  }

  // --- Federation state ---
  let peers = $state<AdminPeerRow[]>([]);
  let peersLoading = $state(false);
  let peersErr = $state<string | null>(null);

  async function loadPeers() {
    peersLoading = true;
    peersErr = null;
    const res = await getAdminPeers(50);
    peersLoading = false;
    if (res.success && res.data) {
      peers = res.data;
    } else if (res.error?.code === 'http_401') {
      denied = 'anon';
    } else if (res.error?.code === 'http_403') {
      denied = 'not-admin';
    } else {
      peersErr = res.error?.message ?? 'Falha ao carregar peers.';
    }
  }

  // --- Moderation state ---
  let hideId = $state('');
  let hideBusy = $state(false);
  let hideResult = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);

  async function hideNote() {
    if (!hideId.trim()) return;
    hideBusy = true;
    hideResult = null;
    const res = await hideAdminNote(hideId.trim());
    hideBusy = false;
    if (res.success) {
      hideResult = {
        kind: 'ok',
        text: `Nota ${hideId.trim().slice(0, 8)}… foi ocultada.`,
      };
      hideId = '';
    } else {
      hideResult = {
        kind: 'error',
        text: res.error?.message ?? 'Não foi possível ocultar a nota.',
      };
    }
  }

  // --- Tab switching ---
  function select(id: string) {
    const t = id as TabId;
    active = t;
    if (typeof history !== 'undefined') {
      history.replaceState(null, '', `#${t}`);
    }
    if (denied) return;
    if (t === 'painel' && stats === null && !statsLoading) loadStats();
    if (t === 'usuarios' && usersRows.length === 0 && !usersLoading)
      loadUsers();
    if (t === 'federacao' && peers.length === 0 && !peersLoading) loadPeers();
  }

  onMount(() => {
    const h = window.location.hash.replace('#', '') as TabId;
    if (tabs.some((tab) => tab.id === h)) active = h;
    // Always try to load stats first — if the caller isn't an admin the
    // response flips `denied` and every other section shows the gate too.
    loadStats();
    if (active !== 'painel') select(active);
  });

  function fmtNum(n: number): string {
    return new Intl.NumberFormat('pt-BR').format(n);
  }
  function fmtDate(iso: string): string {
    try {
      return new Date(iso).toLocaleString('pt-BR', {
        day: '2-digit',
        month: 'short',
        year: 'numeric',
      });
    } catch {
      return iso;
    }
  }
</script>

<div class="console">
  {#if denied === 'anon'}
    <Card padding="none">
      <EmptyState
        icon="lock"
        title="Entre para acessar o admin"
        description="Você precisa estar autenticado na sua conta administradora."
        action={anonAction}
      />
    </Card>
    {#snippet anonAction()}
      <Button href={`/entrar?next=${encodeURIComponent('/admin/')}`} variant="primary">
        Entrar
      </Button>
    {/snippet}
  {:else if denied === 'not-admin'}
    <Card padding="none">
      <EmptyState
        icon="lock"
        title="Acesso restrito"
        description="Esta área é reservada a administradores da instância DemocraciaBR."
      />
    </Card>
  {:else}
    <Tabs {tabs} bind:active onselect={select} />

    <div class="pane" role="tabpanel">
      {#if active === 'painel'}
        {#if statsLoading}
          <div class="grid cards">
            {#each Array(6) as _}
              <Card><Skeleton height="4rem" /></Card>
            {/each}
          </div>
        {:else if statsErr}
          <Alert tone="danger">{statsErr}</Alert>
        {:else if stats}
          <div class="grid cards">
            <Card>
              <div class="metric">
                <span class="k">Cidadãos</span>
                <strong class="v">{fmtNum(stats.citizens)}</strong>
                <span class="s muted">{fmtNum(stats.actors_local)} públicos no fediverso</span>
              </div>
            </Card>
            <Card>
              <div class="metric">
                <span class="k">Atores remotos vistos</span>
                <strong class="v">{fmtNum(stats.actors_remote)}</strong>
                <span class="s muted">Perfis de outras instâncias</span>
              </div>
            </Card>
            <Card>
              <div class="metric">
                <span class="k">Publicações</span>
                <strong class="v">{fmtNum(stats.notes_total)}</strong>
                <span class="s muted">
                  +{fmtNum(stats.notes_last_7d)} nos últimos 7 dias
                </span>
              </div>
            </Card>
            <Card>
              <div class="metric">
                <span class="k">Mandatos</span>
                <strong class="v">{fmtNum(stats.mandates)}</strong>
                <span class="s muted">Câmara + Senado + subnacionais</span>
              </div>
            </Card>
            <Card>
              <div class="metric">
                <span class="k">Propostas</span>
                <strong class="v">{fmtNum(stats.proposals)}</strong>
                <span class="s muted">Total publicado</span>
              </div>
            </Card>
            <Card>
              <div class="metric">
                <span class="k">Notificações não lidas</span>
                <strong class="v">{fmtNum(stats.notifications_unread)}</strong>
                <span class="s muted">Somando todos os cidadãos</span>
              </div>
            </Card>
          </div>
          <div class="foot">
            <Button variant="ghost" size="sm" onclick={loadStats}>
              <Icon name="cw" size={14} /> Atualizar
            </Button>
          </div>
        {/if}
      {:else if active === 'usuarios'}
        <div class="promo-card">
          <p>
            <strong>Nova GUI de usuários</strong> — busca, filtros por partido,
            papel plataforma, papel partido e tipo cívico + drawer de edição.
          </p>
          <a class="btn btn-primary" href="/admin/usuarios">
            Abrir GUI completa →
          </a>
        </div>
        <div class="filters">
          <div class="q-wrap">
            <Input
              id="admin-users-q"
              label=""
              placeholder="Buscar por handle ou e-mail…"
              bind:value={usersQ}
              onkeydown={(e: KeyboardEvent) => e.key === 'Enter' && loadUsers()}
            />
          </div>
          <Button variant="secondary" onclick={() => loadUsers()}>
            <Icon name="search" size={16} /> Buscar
          </Button>
        </div>
        {#if usersLoading && usersRows.length === 0}
          <div class="loading"><Spinner /></div>
        {:else if usersErr}
          <Alert tone="danger">{usersErr}</Alert>
        {:else if usersRows.length === 0}
          <Card padding="none">
            <EmptyState
              icon="users"
              title="Nenhum cidadão encontrado"
              description={usersQ
                ? `Não achamos ninguém com "${usersQ}".`
                : 'A base ainda está vazia.'}
            />
          </Card>
        {:else}
          <div class="table-wrap">
            <table>
              <thead>
                <tr>
                  <th>Cidadão</th>
                  <th>E-mail</th>
                  <th>Verificação</th>
                  <th>Público</th>
                  <th>Criado</th>
                  <th>Papel</th>
                </tr>
              </thead>
              <tbody>
                {#each usersRows as u (u.citizen_id)}
                  <tr>
                    <td>
                      <div class="who">
                        <strong>{u.display_name || u.handle || u.citizen_id.slice(0, 8)}</strong>
                        {#if u.handle}<span class="muted">@{u.handle}</span>{/if}
                      </div>
                    </td>
                    <td class="mono">{u.email || '—'}</td>
                    <td><Badge tone="neutral">{u.verification_level}</Badge></td>
                    <td>
                      {#if u.is_public}
                        <Badge tone="success">sim</Badge>
                      {:else}
                        <Badge tone="neutral">não</Badge>
                      {/if}
                    </td>
                    <td class="muted">{fmtDate(u.created_at)}</td>
                    <td>
                      <select
                        value={u.role ?? ''}
                        disabled={roleBusy === u.citizen_id}
                        onchange={(e) =>
                          changeRole(
                            u,
                            (e.currentTarget as HTMLSelectElement).value || null,
                          )}
                      >
                        <option value="">— nenhum —</option>
                        <option value="auditor">auditor</option>
                        <option value="admin">admin</option>
                        <option value="owner">owner</option>
                      </select>
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
          {#if usersHasMore}
            <div class="foot">
              <Button
                variant="ghost"
                onclick={() => {
                  usersOffset += PAGE;
                  loadUsers(false);
                }}
                loading={usersLoading}
              >
                Carregar mais
              </Button>
            </div>
          {/if}
        {/if}
      {:else if active === 'federacao'}
        <div class="promo-card">
          <p>
            <strong>Bloqueios de domínio server-wide</strong> — política da
            instância inteira: silenciar (só quem já segue continua vendo) ou
            suspender (corte total, inbox rejeita).
          </p>
          <a class="btn btn-primary" href="/admin/federacao">
            Gerenciar bloqueios de domínio →
          </a>
        </div>
        {#if peersLoading}
          <div class="loading"><Spinner /></div>
        {:else if peersErr}
          <Alert tone="danger">{peersErr}</Alert>
        {:else if peers.length === 0}
          <Card padding="none">
            <EmptyState
              icon="globe"
              title="Nenhuma instância vizinha"
              description="Assim que alguém desta instância seguir um perfil de outra, o host aparece aqui."
            />
          </Card>
        {:else}
          <div class="table-wrap">
            <table>
              <thead>
                <tr>
                  <th>Instância</th>
                  <th>Atores conhecidos</th>
                  <th>Última publicação vista</th>
                </tr>
              </thead>
              <tbody>
                {#each peers as p (p.host)}
                  <tr>
                    <td>
                      <a href={`https://${p.host}`} target="_blank" rel="noopener noreferrer">
                        {p.host} <Icon name="external" size={12} />
                      </a>
                    </td>
                    <td class="mono">{fmtNum(p.actor_count)}</td>
                    <td class="muted">{fmtDate(p.last_seen)}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
          <div class="foot">
            <Button variant="ghost" size="sm" onclick={loadPeers}>
              <Icon name="cw" size={14} /> Atualizar
            </Button>
          </div>
        {/if}
      {:else if active === 'moderacao'}
        <div class="promo-card">
          <p>
            <strong>Fila de denúncias</strong> — publicações reportadas por
            cidadãos, com filtro por status (pendentes / resolvidas), notas do
            moderador e histórico.
          </p>
          <a class="btn btn-primary" href="/admin/denuncias">
            Abrir fila de denúncias →
          </a>
        </div>
        <Card>
          <h3 class="sub"><Icon name="trash" size={18} /> Ocultar uma publicação</h3>
          <p class="muted">
            Cole o ID (UUID) da nota. A publicação é marcada como excluída
            imediatamente. Só notas locais podem ser ocultas por aqui — remotas
            precisam ser reportadas à instância de origem.
          </p>
          <div class="hide-form">
            <div class="q-wrap">
              <Input
                id="admin-hide-id"
                label=""
                placeholder="00000000-0000-0000-0000-000000000000"
                bind:value={hideId}
              />
            </div>
            <Button
              variant="danger"
              onclick={hideNote}
              loading={hideBusy}
              disabled={!hideId.trim()}
            >
              Ocultar
            </Button>
          </div>
          {#if hideResult}
            <div class="alert-slot">
              <Alert tone={hideResult.kind === 'ok' ? 'success' : 'danger'}>
                {hideResult.text}
              </Alert>
            </div>
          {/if}
        </Card>
      {:else if active === 'emails'}
        <EmailTemplatesAdmin />
      {/if}
    </div>
  {/if}
</div>

<style>
  .promo-card {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: var(--sp-4);
    padding: var(--sp-3) var(--sp-4);
    background: var(--accent-soft);
    border: 1px solid var(--accent);
    border-radius: var(--r-base);
    margin-bottom: var(--sp-4);
    flex-wrap: wrap;
  }
  .promo-card p {
    margin: 0;
    color: var(--text-1);
  }
  .promo-card .btn {
    background: var(--accent);
    color: var(--accent-contrast);
    padding: 8px 16px;
    border-radius: var(--r-sm);
    text-decoration: none;
    font-weight: var(--fw-semibold);
    white-space: nowrap;
  }
  .console {
    display: block;
  }
  .pane {
    padding-top: var(--sp-5);
  }
  .grid {
    display: grid;
    gap: var(--sp-4);
  }
  .cards {
    grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
  }
  .metric {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .metric .k {
    color: var(--text-3);
    font-size: var(--fs-sm);
    font-weight: var(--fw-medium);
  }
  .metric .v {
    color: var(--text-1);
    font-size: var(--fs-3xl);
    font-variant-numeric: tabular-nums;
    line-height: 1;
  }
  .metric .s {
    font-size: var(--fs-xs);
  }
  .loading {
    display: flex;
    justify-content: center;
    padding: var(--sp-8);
  }
  .filters {
    display: flex;
    gap: var(--sp-3);
    align-items: flex-end;
    margin-bottom: var(--sp-4);
    flex-wrap: wrap;
  }
  .q-wrap {
    flex: 1;
    min-width: 240px;
  }
  .table-wrap {
    overflow-x: auto;
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-base);
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--fs-sm);
  }
  th,
  td {
    padding: var(--sp-3) var(--sp-4);
    text-align: left;
    border-bottom: 1px solid var(--border-subtle);
    vertical-align: middle;
  }
  th {
    background: var(--surface-2);
    color: var(--text-3);
    font-weight: var(--fw-semibold);
    font-size: var(--fs-xs);
    text-transform: uppercase;
    letter-spacing: 0.03em;
  }
  tbody tr:last-child td {
    border-bottom: 0;
  }
  tbody tr:hover {
    background: var(--surface-2);
  }
  .mono {
    font-family: ui-monospace, SFMono-Regular, monospace;
    font-size: var(--fs-xs);
  }
  .who {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .who strong {
    color: var(--text-1);
  }
  .who .muted {
    font-size: var(--fs-xs);
  }
  select {
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    color: var(--text-1);
    padding: var(--sp-1) var(--sp-2);
    border-radius: var(--r-sm);
    font: inherit;
    font-size: var(--fs-sm);
    cursor: pointer;
  }
  .foot {
    display: flex;
    justify-content: center;
    padding-top: var(--sp-4);
  }
  .sub {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    margin: 0 0 var(--sp-3);
    font-size: var(--fs-lg);
    color: var(--text-1);
  }
  .hide-form {
    display: flex;
    gap: var(--sp-2);
    align-items: flex-end;
    margin-top: var(--sp-3);
  }
  .alert-slot {
    margin-top: var(--sp-3);
  }
  a {
    color: var(--accent);
    text-decoration: none;
  }
  a:hover {
    text-decoration: underline;
  }
</style>
