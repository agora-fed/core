<script lang="ts">
  // /rede — tabs 'Seguindo' e 'Seguidores'. Cada card mostra handle inferido
  // do actor_url, timestamp de "since" e badge Aguardando ACK quando o Follow
  // ainda não teve accepted_at (não confirmado remoto).
  import { onMount } from 'svelte';
  import { listMyFollowing, listMyFollowers, type SocialLinkDto } from '../../lib/api';
  import { formatDate, formatRelative } from '../../lib/format';
  import Card from '../ui/Card.svelte';
  import Badge from '../ui/Badge.svelte';
  import EmptyState from '../ui/EmptyState.svelte';

  type Tab = 'following' | 'followers';
  let tab = $state<Tab>('following');
  let items = $state<SocialLinkDto[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);

  async function reload() {
    loading = true;
    error = null;
    const res = tab === 'following' ? await listMyFollowing() : await listMyFollowers();
    loading = false;
    if (res.success && res.data) {
      items = res.data;
    } else {
      error = res.error?.message ?? 'Falha ao carregar.';
    }
  }

  onMount(reload);

  function pick(t: Tab) {
    if (t === tab) return;
    tab = t;
    reload();
  }

  function profileHref(handle_hint: string | null, actor_url: string): string {
    if (handle_hint) return `/perfil/?u=${encodeURIComponent(handle_hint)}`;
    // Fallback: passa o próprio actor_url como u.
    return `/perfil/?u=${encodeURIComponent(actor_url)}`;
  }
</script>

<nav class="tabs" aria-label="Alternar entre seguindo e seguidores">
  <button type="button" class:on={tab === 'following'} onclick={() => pick('following')}>
    Seguindo
  </button>
  <button type="button" class:on={tab === 'followers'} onclick={() => pick('followers')}>
    Seguidores
  </button>
</nav>

{#if loading}
  <p class="muted">Carregando…</p>
{:else if error}
  <Card>
    <EmptyState icon="users" title="Erro" description={error} />
  </Card>
{:else if items.length === 0}
  <Card>
    <EmptyState
      icon="users"
      title={tab === 'following' ? 'Você ainda não segue ninguém' : 'Sem seguidores por enquanto'}
      description={tab === 'following'
        ? 'Vá em Explorar e encontre pessoas pra acompanhar. Contas do fediverso também aparecem.'
        : 'Assim que alguém te seguir (daqui ou de outra instância), a lista se preenche.'}
    />
  </Card>
{:else}
  <ul class="rows">
    {#each items as it (it.actor_url)}
      <li>
        <Card>
          <div class="row">
            <div class="info">
              <a class="who" href={profileHref(it.handle_hint, it.actor_url)}>
                <strong>{it.handle_hint ?? it.actor_url}</strong>
              </a>
              <div class="meta muted">
                {formatRelative(it.since)}
                {#if !it.accepted}
                  · <Badge tone="warning" size="sm">Aguardando confirmação</Badge>
                {/if}
              </div>
            </div>
          </div>
        </Card>
      </li>
    {/each}
  </ul>
{/if}

<style>
  .tabs {
    display: flex;
    gap: var(--sp-1);
    margin-bottom: var(--sp-4);
    border-bottom: 1px solid var(--border-subtle);
  }
  .tabs button {
    background: transparent;
    border: 0;
    padding: var(--sp-2) var(--sp-3);
    font: inherit;
    font-size: var(--fs-sm);
    font-weight: var(--fw-medium);
    color: var(--text-2);
    cursor: pointer;
    border-bottom: 2px solid transparent;
    margin-bottom: -1px;
  }
  .tabs button:hover {
    color: var(--text-1);
  }
  .tabs button.on {
    color: var(--accent-strong);
    border-bottom-color: var(--accent-strong);
    font-weight: var(--fw-semibold);
  }
  .rows {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: var(--sp-2);
  }
  .row {
    display: flex;
    gap: var(--sp-3);
    align-items: center;
  }
  .info {
    flex: 1;
    min-width: 0;
  }
  .who {
    text-decoration: none;
    color: var(--text-1);
    font-family: ui-monospace, SFMono-Regular, monospace;
    font-size: var(--fs-sm);
  }
  .who:hover {
    color: var(--accent-strong);
    text-decoration: underline;
  }
  .meta {
    font-size: var(--fs-xs);
    margin-top: 2px;
  }
</style>
