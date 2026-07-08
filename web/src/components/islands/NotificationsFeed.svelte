<script lang="ts">
  // In-app notifications feed. Types: mention, reply, favourite, reblog, follow.
  // Each item shows the actor avatar + a kind-specific icon + the object preview
  // (when relevant). Clear-all button POSTs to /me/notifications/clear.
  import { onMount } from 'svelte';
  import {
    getMyNotifications,
    clearMyNotifications,
    isAuthError,
    clearLocalSession,
    type NotificationDto,
  } from '../../lib/api';
  import { enablePush, disablePush, isSubscribed, pushStatus } from '../../lib/webpush';
  import { formatRelative, formatDate } from '../../lib/format';
  import { toast } from '../../lib/toasts';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';
  import Avatar from '../ui/Avatar.svelte';
  import Icon from '../ui/Icon.svelte';
  import Badge from '../ui/Badge.svelte';
  import Skeleton from '../ui/Skeleton.svelte';
  import EmptyState from '../ui/EmptyState.svelte';
  import ErrorState from '../ui/ErrorState.svelte';

  const PAGE = 30;

  type Kind = NotificationDto['kind'];

  // Kinds cívicas (migration 0411) — feed do 'propor → threshold → SLA → resposta'.
  // Fallback pra kind desconhecido usa 'info' + 'bell' pra sobreviver a novas kinds
  // servidas antes do front deployar.
  const iconByKind: Record<string, string> = {
    mention: 'at',
    reply: 'reply',
    favourite: 'heart-fill',
    reblog: 'boost',
    follow: 'profile',
    proposal_threshold: 'bell',
    sla_started: 'bell',
    sla_response: 'check',
    sla_expired: 'alert',
  };
  const labelByKind: Record<string, string> = {
    mention: 'mencionou você',
    reply: 'respondeu a você',
    favourite: 'favoritou seu post',
    reblog: 'repostou seu post',
    follow: 'começou a seguir você',
    proposal_threshold: 'sua proposta cruzou o gatilho',
    sla_started: 'SLA do mandato começou',
    sla_response: 'o mandato respondeu você',
    sla_expired: 'silêncio público registrado',
  };
  const toneByKind: Record<
    string,
    'accent' | 'danger' | 'info' | 'success' | 'warning'
  > = {
    mention: 'accent',
    reply: 'info',
    favourite: 'danger',
    reblog: 'success',
    follow: 'accent',
    proposal_threshold: 'success',
    sla_started: 'info',
    sla_response: 'success',
    sla_expired: 'warning',
  };

  let ready = $state(false);
  let loggedIn = $state(false);
  let loading = $state(true);
  let items = $state<NotificationDto[]>([]);
  let unread = $state(0);
  let loadError = $state<string | null>(null);
  let busy = $state(false);
  // Push notifications state.
  let pushSupported = $state(false);
  let pushOn = $state(false);
  let pushBusy = $state(false);
  let pushMsg = $state<string | null>(null);

  function isLogged(): boolean {
    try {
      return !!localStorage.getItem('dsoc_citizen');
    } catch {
      return false;
    }
  }

  async function load() {
    loading = true;
    loadError = null;
    const res = await getMyNotifications(PAGE, 0);
    loading = false;
    if (res.success && res.data) {
      items = res.data.items;
      unread = res.data.unread_count;
    } else if (isAuthError(res)) {
      clearLocalSession();
      loggedIn = false;
    } else {
      loadError = res.error?.message ?? 'Não foi possível carregar as notificações.';
    }
  }

  async function clearAll() {
    if (busy || unread === 0) return;
    busy = true;
    const res = await clearMyNotifications();
    busy = false;
    if (res.success && res.data) {
      toast.success(
        res.data.cleared === 0
          ? 'Nada por fazer.'
          : `${res.data.cleared} ${res.data.cleared === 1 ? 'notificação marcada' : 'notificações marcadas'} como lidas.`,
      );
      // Optimistic: flip local state so the badge zeroes right away.
      items = items.map((i) => ({ ...i, read: true }));
      unread = 0;
    } else {
      toast.error(res.error?.message ?? 'Não foi possível marcar como lidas.');
    }
  }

  onMount(async () => {
    loggedIn = isLogged();
    ready = true;
    if (loggedIn) void load();
    else loading = false;
    pushSupported = pushStatus() !== 'unsupported';
    if (pushSupported) pushOn = await isSubscribed();
  });

  async function togglePush() {
    if (pushBusy) return;
    pushBusy = true;
    pushMsg = null;
    if (pushOn) {
      await disablePush();
      pushOn = false;
      pushMsg = 'Notificações push desativadas neste dispositivo.';
    } else {
      const r = await enablePush();
      if (r.ok) {
        pushOn = true;
        pushMsg = 'Notificações push ativadas neste dispositivo.';
      } else {
        pushMsg = r.reason ?? 'Não foi possível ativar.';
      }
    }
    pushBusy = false;
  }

  const CIVIC_KINDS = new Set([
    'proposal_threshold',
    'sla_started',
    'sla_response',
    'sla_expired',
  ]);

  function targetHref(item: NotificationDto): string | null {
    if (item.kind === 'follow') return item.source_actor_url;
    // Kinds cívicas: object_uri já vem como URL completa `/propostas/<id>`.
    // Reutilizar direto — /publicacao?uri= é só pro fediverso.
    if (item.object_uri && CIVIC_KINDS.has(item.kind)) {
      return item.object_uri;
    }
    if (item.object_uri) {
      return `/publicacao?uri=${encodeURIComponent(item.object_uri)}`;
    }
    return null;
  }
</script>

{#if !ready}
  <p class="muted" aria-hidden="true">Carregando…</p>
{:else if !loggedIn}
  <Card padding="none">
    <EmptyState
      icon="bell"
      title="Entre para ver suas notificações"
      description="Você precisa estar logado para acompanhar quem interagiu com suas publicações."
      action={loginAction}
    />
    {#snippet loginAction()}
      <Button href="/entrar" variant="primary">Entrar</Button>
    {/snippet}
  </Card>
{:else}
  <header class="head">
    <div>
      <h2>Notificações</h2>
      {#if unread > 0}
        <Badge tone="accent" size="sm">{unread} novas</Badge>
      {/if}
    </div>
    <div class="head-actions">
      {#if pushSupported}
        <Button
          variant={pushOn ? 'primary' : 'ghost'}
          size="sm"
          onclick={togglePush}
          loading={pushBusy}
          title={pushOn ? 'Notificações push ativadas neste dispositivo' : 'Ativar notificações push neste dispositivo'}
        >
          <Icon name="bell" size={14} />
          {pushOn ? 'Push ativado' : 'Ativar push'}
        </Button>
      {/if}
      <Button
        variant="ghost"
        size="sm"
        onclick={clearAll}
        disabled={unread === 0 || busy}
        loading={busy}
      >
        <Icon name="check" size={14} />
        Marcar todas como lidas
      </Button>
    </div>
  </header>
  {#if pushMsg}
    <p class="push-msg muted">{pushMsg}</p>
  {/if}

  {#if loading}
    <div class="skeletons">
      {#each [0, 1, 2] as i (i)}
        <Card>
          <div class="sk">
            <Skeleton variant="circle" width="40px" />
            <div style="flex:1">
              <Skeleton width="60%" />
              <Skeleton width="30%" />
            </div>
          </div>
        </Card>
      {/each}
    </div>
  {:else if loadError}
    <ErrorState
      title="Não foi possível carregar"
      message={loadError}
      retry={load}
    />
  {:else if items.length === 0}
    <Card padding="none">
      <EmptyState
        icon="bell"
        title="Sem novidades"
        description="Quando alguém curtir, respostar, mencionar ou seguir você, aparece aqui."
      />
    </Card>
  {:else}
    <ol class="list">
      {#each items as item (item.id)}
        {@const href = targetHref(item)}
        <li>
          <Card>
            <div class="notif" class:unread={!item.read}>
              <div class="who">
                <Avatar
                  src={item.source_avatar_url}
                  name={item.source_display_name ?? item.source_handle}
                  alt=""
                  size="sm"
                />
                <span class="kind" data-tone={toneByKind[item.kind] ?? 'info'}>
                  <Icon name={iconByKind[item.kind] ?? 'bell'} size={14} />
                </span>
              </div>
              <div class="body">
                <p class="line">
                  <strong>
                    {item.source_display_name ?? `@${item.source_handle}`}
                  </strong>
                  <span class="muted">{labelByKind[item.kind] ?? item.kind}</span>
                </p>
                {#if item.object_preview}
                  <p class="preview">{item.object_preview}</p>
                {/if}
                <time
                  class="when muted"
                  datetime={item.created_at}
                  title={formatDate(item.created_at)}
                >
                  {formatRelative(item.created_at)}
                </time>
              </div>
              {#if href}
                <a class="stretch" href={href} aria-label="Abrir">
                  <Icon name="arrow-right" size={16} />
                </a>
              {/if}
            </div>
          </Card>
        </li>
      {/each}
    </ol>
  {/if}
{/if}

<style>
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--sp-3);
    margin-bottom: var(--sp-4);
    flex-wrap: wrap;
  }
  .head > div {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
  }
  .head h2 {
    margin: 0;
    font-size: var(--fs-xl);
    color: var(--text-1);
  }
  .head-actions {
    display: flex;
    gap: var(--sp-2);
    flex-wrap: wrap;
    align-items: center;
  }
  .push-msg {
    margin: 0 0 var(--sp-4);
    padding: var(--sp-2) var(--sp-3);
    background: var(--surface-2);
    border-radius: var(--r-sm);
    font-size: var(--fs-sm);
  }
  .list {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: var(--sp-2);
  }
  .notif {
    display: flex;
    align-items: flex-start;
    gap: var(--sp-3);
    position: relative;
  }
  .notif.unread {
    padding-left: var(--sp-2);
    margin-left: calc(-1 * var(--sp-2));
    border-left: 3px solid var(--accent);
  }
  .who {
    position: relative;
    flex-shrink: 0;
  }
  .kind {
    position: absolute;
    right: -4px;
    bottom: -4px;
    width: 20px;
    height: 20px;
    border-radius: 50%;
    background: var(--surface-1);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    box-shadow: var(--shadow-sm);
  }
  .kind[data-tone='accent'] {
    color: var(--accent);
  }
  .kind[data-tone='danger'] {
    color: var(--danger);
  }
  .kind[data-tone='info'] {
    color: var(--info);
  }
  .kind[data-tone='success'] {
    color: var(--success);
  }
  .kind[data-tone='warning'] {
    color: var(--warning);
  }
  .body {
    flex: 1;
    min-width: 0;
  }
  .line {
    margin: 0 0 2px;
    font-size: var(--fs-sm);
    color: var(--text-1);
  }
  .line strong {
    font-weight: var(--fw-semibold);
    margin-right: var(--sp-1);
  }
  .preview {
    margin: 0 0 var(--sp-1);
    font-size: var(--fs-sm);
    color: var(--text-2);
    line-height: var(--lh-snug);
    overflow: hidden;
    text-overflow: ellipsis;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
  }
  .when {
    font-size: var(--fs-xs);
  }
  .stretch {
    position: absolute;
    inset: calc(-1 * var(--sp-4));
    display: flex;
    align-items: center;
    justify-content: flex-end;
    padding-right: var(--sp-4);
    color: var(--text-3);
    text-decoration: none;
    border-radius: var(--r-base);
  }
  .stretch:hover {
    background: color-mix(in srgb, var(--surface-2) 60%, transparent);
  }
  .skeletons {
    display: grid;
    gap: var(--sp-2);
  }
  .sk {
    display: flex;
    gap: var(--sp-3);
    align-items: center;
  }
  .muted {
    color: var(--text-3);
  }
</style>
