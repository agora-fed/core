<script lang="ts">
  // OAuth apps the caller has authorized. One card per app, with a Revoke
  // button that kills every live bearer token this citizen has issued to it
  // (a Tusky reinstall reappears here after logging back in).
  import { onMount } from 'svelte';
  import {
    getAuthorizedApps,
    revokeAuthorizedApp,
    type AuthorizedAppDto,
  } from '../../lib/api';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';
  import Badge from '../ui/Badge.svelte';
  import Icon from '../ui/Icon.svelte';
  import Alert from '../ui/Alert.svelte';
  import EmptyState from '../ui/EmptyState.svelte';
  import Spinner from '../ui/Spinner.svelte';

  let loading = $state(true);
  let apps = $state<AuthorizedAppDto[]>([]);
  let loadError = $state<string | null>(null);
  let busyId = $state<string | null>(null);
  let toast = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);

  async function reload() {
    const res = await getAuthorizedApps();
    if (res.success && res.data) {
      apps = res.data;
      loadError = null;
    } else if (res.error?.code === 'http_401') {
      loadError = 'Entre na sua conta para ver os apps conectados.';
    } else {
      loadError =
        res.error?.message ?? 'Não foi possível carregar os apps conectados.';
    }
  }

  onMount(async () => {
    await reload();
    loading = false;
  });

  async function revoke(app: AuthorizedAppDto) {
    if (!confirm(`Desconectar "${app.name}"? O app precisará entrar de novo.`)) {
      return;
    }
    busyId = app.application_id;
    toast = null;
    const res = await revokeAuthorizedApp(app.application_id);
    busyId = null;
    if (res.success) {
      apps = apps.filter((a) => a.application_id !== app.application_id);
      toast = { kind: 'ok', text: `"${app.name}" foi desconectado.` };
    } else {
      toast = {
        kind: 'error',
        text: res.error?.message ?? 'Não foi possível desconectar.',
      };
    }
  }

  function fmt(iso: string): string {
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

<section>
  <p class="lede">
    Apps do fediverso (Tusky, Elk, Ice Cubes, Ivory e afins) que você conectou
    à sua conta. Desconectar aqui revoga o acesso do app à API.
  </p>

  {#if loading}
    <div class="loading"><Spinner /></div>
  {:else if loadError}
    <Alert tone="danger">{loadError}</Alert>
  {:else if apps.length === 0}
    <Card padding="none">
      <EmptyState
        icon="link"
        title="Nenhum app conectado"
        description="Quando você entrar em um app compatível com Mastodon e autorizar acesso a esta conta, ele aparece aqui."
      />
    </Card>
  {:else}
    <ul class="list">
      {#each apps as app (app.application_id)}
        <li>
          <Card>
            <div class="row">
              <div class="head">
                <div class="ico" aria-hidden="true">
                  <Icon name="link" size={22} />
                </div>
                <div class="meta">
                  <strong class="name">{app.name}</strong>
                  {#if app.website}
                    <a class="site" href={app.website} target="_blank" rel="noopener noreferrer">
                      {app.website.replace(/^https?:\/\//, '')}
                    </a>
                  {/if}
                  <div class="tags">
                    {#each app.scopes.split(/\s+/).filter(Boolean) as s}
                      <Badge tone="neutral">{s}</Badge>
                    {/each}
                  </div>
                  <span class="muted since">
                    Autorizado em {fmt(app.first_authorized_at)} · expira em {fmt(app.last_expires_at)}
                  </span>
                </div>
              </div>
              <Button
                variant="ghost"
                size="sm"
                loading={busyId === app.application_id}
                disabled={busyId !== null}
                onclick={() => revoke(app)}
              >
                Desconectar
              </Button>
            </div>
          </Card>
        </li>
      {/each}
    </ul>
  {/if}

  {#if toast}
    <div class="toast-slot">
      <Alert tone={toast.kind === 'ok' ? 'success' : 'danger'}>{toast.text}</Alert>
    </div>
  {/if}
</section>

<style>
  .lede {
    color: var(--text-3);
    margin: 0 0 var(--sp-4);
  }
  .loading {
    display: flex;
    justify-content: center;
    padding: var(--sp-6);
  }
  .list {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: var(--sp-3);
  }
  .row {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: var(--sp-4);
  }
  .head {
    display: flex;
    align-items: flex-start;
    gap: var(--sp-3);
    flex: 1;
    min-width: 0;
  }
  .ico {
    width: 40px;
    height: 40px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: var(--accent-soft);
    color: var(--accent);
    border-radius: var(--r-base);
    flex-shrink: 0;
  }
  .meta {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }
  .name {
    font-size: var(--fs-base);
    color: var(--text-1);
  }
  .site {
    font-size: var(--fs-sm);
    color: var(--accent);
    text-decoration: none;
  }
  .site:hover {
    text-decoration: underline;
  }
  .tags {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin-top: 2px;
  }
  .since {
    font-size: var(--fs-xs);
    margin-top: 2px;
  }
  .toast-slot {
    margin-top: var(--sp-3);
  }
</style>
