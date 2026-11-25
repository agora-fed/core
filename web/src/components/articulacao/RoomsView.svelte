<script lang="ts">
  import { onMount } from 'svelte';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';
  import Input from '../ui/Input.svelte';
  import Badge from '../ui/Badge.svelte';
  import EmptyState from '../ui/EmptyState.svelte';
  import Skeleton from '../ui/Skeleton.svelte';
  import Failure from './Failure.svelte';
  import { toast } from '../../lib/toasts';
  import { api, failureOf, href, roleLabel, type Failure as F, type MyRooms } from '../../lib/articulacao';

  let data = $state<MyRooms | null>(null);
  let failure = $state<F | null>(null);
  let name = $state('');
  let busy = $state(false);

  async function load() {
    failure = null;
    const res = await api.rooms();
    if (res.success && res.data) data = res.data;
    else failure = failureOf(res);
  }
  onMount(load);

  async function create(e: SubmitEvent) {
    e.preventDefault();
    busy = true;
    const res = await api.createRoom(name);
    busy = false;
    if (!res.success || !res.data) {
      toast.error(res.error?.message ?? 'Não foi possível criar a sala.');
      return;
    }
    window.location.hash = href.room(res.data.id);
  }
</script>

<header class="art-head">
  <h1>Suas salas</h1>
  {#if data}<span class="muted small">Você: {data.display_name}</span>{/if}
</header>

{#if failure}
  <Failure {failure} retry={load} />
{:else if !data}
  <Skeleton variant="block" height="6rem" />
{:else}
  {#if data.can_create_rooms}
    <Card>
      <form class="create" onsubmit={create}>
        <div class="grow">
          <Input label="Nome da nova sala" bind:value={name} required maxlength={120} placeholder="Ex.: Reforma tributária" />
        </div>
        <Button type="submit" variant="primary" loading={busy} disabled={!name.trim()}>Criar sala</Button>
      </form>
    </Card>
  {:else}
    <p class="muted small">
      Para criar uma sala, peça a permissão a quem administra o site. Você também pode ser
      convidada(o) para uma sala existente.
    </p>
  {/if}

  {#if data.rooms.length === 0}
    <Card padding="none">
      <EmptyState icon="users" title="Nenhuma sala ainda" description="Quando alguém adicionar você a uma sala, ela aparece aqui." />
    </Card>
  {:else}
    <ul class="rooms">
      {#each data.rooms as r (r.id)}
        <li>
          <a class="room" href={href.room(r.id)}>
            <span class="room-name">{r.name}</span>
            <Badge tone={r.role === 'owner' ? 'accent' : 'neutral'} size="sm">{roleLabel(r.role)}</Badge>
          </a>
        </li>
      {/each}
    </ul>
  {/if}
{/if}

<style>
  .small {
    font-size: var(--fs-sm);
  }
  .create {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-end;
    gap: var(--sp-3);
  }
  .create :global(.field) {
    margin-bottom: 0;
  }
  .grow {
    flex: 1 1 18rem;
    min-width: 0;
  }
  .rooms {
    list-style: none;
    padding: 0;
    margin: var(--sp-5) 0 0;
    display: grid;
    gap: var(--sp-3);
    grid-template-columns: repeat(auto-fill, minmax(min(100%, 18rem), 1fr));
  }
  .room {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--sp-3);
    padding: var(--sp-4) var(--sp-5);
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-base);
    box-shadow: var(--shadow-sm);
    color: var(--text-1);
    text-decoration: none;
    transition: border-color var(--dur-fast) var(--ease-out);
  }
  .room:hover {
    border-color: var(--accent);
  }
  .room-name {
    font-weight: var(--fw-semibold);
    min-width: 0;
    overflow-wrap: anywhere;
  }
</style>
