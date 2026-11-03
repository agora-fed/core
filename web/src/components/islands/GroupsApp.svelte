<script lang="ts">
  // /grupos: the list, and /grupos?g=<slug> for one group. Static page, so the slug is read here.
  import { onMount } from 'svelte';
  import {
    addGroupMember,
    archiveGroup,
    canManage,
    canSeeMembers,
    createGroup,
    enableGroupForum,
    getGroup,
    groupMembers,
    joinGroup,
    leaveGroup,
    listGroups,
    removeGroupMember,
    searchPeople,
    updateGroup,
    updateGroupMember,
    type GroupDto,
    type GroupMemberDto,
    type GroupPersonDto,
  } from '../../lib/groups';
  import { toast } from '../../lib/toasts';

  let loading = $state(true);
  let signedIn = $state(false);
  let isAdmin = $state(false);
  let groups = $state<GroupDto[]>([]);
  let group = $state<GroupDto | null>(null);
  let missing = $state(false);
  let members = $state<GroupMemberDto[]>([]);
  let draft = $state({ slug: '', name: '', description: '', entry: 'approval' as 'open' | 'approval' });
  let personQuery = $state('');
  let people = $state<GroupPersonDto[]>([]);
  let busy = $state(false);

  const manager = $derived(group ? canManage(group, isAdmin) : false);
  const pending = $derived(members.filter((m) => m.status === 'pending'));
  const active = $derived(members.filter((m) => m.status === 'active'));

  const nameOf = (p: { display_name: string | null; handle: string | null }) =>
    p.display_name ?? (p.handle ? `@${p.handle}` : 'Pessoa sem nome');

  async function refresh(slug: string) {
    const res = await getGroup(slug);
    if (!res.success || !res.data) {
      missing = true;
      group = null;
      return;
    }
    group = res.data;
    members = [];
    if (canSeeMembers(group, isAdmin)) {
      const list = await groupMembers(group.id);
      if (list.success && list.data) members = list.data;
    }
  }

  async function run(action: () => Promise<{ success: boolean; error?: { message: string } | null }>) {
    busy = true;
    const res = await action();
    busy = false;
    if (!res.success) {
      toast.error(res.error?.message ?? 'Não foi possível concluir.');
      return false;
    }
    if (group) await refresh(group.slug);
    return true;
  }

  async function create() {
    busy = true;
    const res = await createGroup(draft);
    busy = false;
    if (!res.success || !res.data) {
      toast.error(res.error?.message ?? 'Não foi possível criar o grupo.');
      return;
    }
    window.location.href = `/grupos?g=${encodeURIComponent(res.data.slug)}`;
  }

  async function search(id: string) {
    const res = await searchPeople(id, personQuery.trim());
    people = res.success && res.data ? res.data : [];
  }

  async function archive(id: string) {
    if (!confirm('Arquivar este grupo? O fórum dele também deixa de aparecer.')) return;
    if (await run(() => archiveGroup(id))) window.location.href = '/grupos';
  }

  onMount(async () => {
    try {
      signedIn = Boolean(localStorage.getItem('dsoc_citizen'));
      isAdmin = localStorage.getItem('dsoc_is_admin') === '1';
    } catch {
      signedIn = false;
    }
    if (signedIn) {
      const slug = new URLSearchParams(window.location.search).get('g');
      if (slug) {
        await refresh(slug);
      } else {
        const res = await listGroups();
        if (res.success && res.data) groups = res.data;
      }
    }
    loading = false;
  });
</script>

{#if loading}
  <p class="muted">Carregando…</p>
{:else if !signedIn}
  <h1>Grupos</h1>
  <p>Entre na sua conta para ver os grupos da comunidade.</p>
  <a class="btn btn-primary" href="/entrar/?next=/grupos">Entrar</a>
{:else if missing}
  <h1>Grupo não encontrado</h1>
  <p><a href="/grupos">Ver todos os grupos</a></p>
{:else if group}
  {@const g = group}
  <p><a href="/grupos">← Grupos</a></p>
  <h1>{g.name}</h1>
  {#if g.description}<p>{g.description}</p>{/if}
  <p class="muted small">
    {g.member_count} {g.member_count === 1 ? 'participante' : 'participantes'} ·
    {g.entry === 'open' ? 'entrada aberta' : 'entrada com aprovação'}
  </p>

  <div class="row">
    {#if g.my_status === 'active'}
      <button class="btn" disabled={busy} onclick={() => run(() => leaveGroup(g.id))}>Sair do grupo</button>
    {:else if g.my_status === 'pending'}
      <span class="muted">Pedido enviado, aguardando a coordenação.</span>
      <button class="btn" disabled={busy} onclick={() => run(() => leaveGroup(g.id))}>Cancelar pedido</button>
    {:else}
      <button class="btn btn-primary" disabled={busy} onclick={() => run(() => joinGroup(g.id))}>
        {g.entry === 'open' ? 'Entrar no grupo' : 'Pedir para entrar'}
      </button>
    {/if}
    {#if g.forum_path}
      <a class="btn" href={`/f/${g.forum_path}`}>Abrir fórum privado</a>
    {/if}
  </div>

  {#if manager && pending.length > 0}
    <h2>Pedidos de entrada</h2>
    <ul class="people">
      {#each pending as m (m.citizen_id)}
        <li>
          <span>{nameOf(m)}</span>
          <button class="btn" disabled={busy} onclick={() => run(() => updateGroupMember(g.id, m.citizen_id, { status: 'active' }))}>Aprovar</button>
          <button class="btn" disabled={busy} onclick={() => run(() => removeGroupMember(g.id, m.citizen_id))}>Recusar</button>
        </li>
      {/each}
    </ul>
  {/if}

  {#if active.length > 0}
    <h2>Quem participa</h2>
    <ul class="people">
      {#each active as m (m.citizen_id)}
        <li>
          <span>{nameOf(m)}{m.role === 'coordinator' ? ' · coordenação' : ''}</span>
          {#if manager}
            <button class="btn" disabled={busy} onclick={() => run(() => updateGroupMember(g.id, m.citizen_id, { role: m.role === 'coordinator' ? 'member' : 'coordinator' }))}>
              {m.role === 'coordinator' ? 'Tirar da coordenação' : 'Pôr na coordenação'}
            </button>
            <button class="btn" disabled={busy} onclick={() => run(() => removeGroupMember(g.id, m.citizen_id))}>Remover</button>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}

  {#if manager}
    <h2>Coordenação</h2>
    <form class="row" onsubmit={(e) => { e.preventDefault(); void search(g.id); }}>
      <input class="input" type="search" minlength="3" placeholder="Nome ou @ (mínimo 3 letras)" bind:value={personQuery} />
      <button class="btn" type="submit" disabled={personQuery.trim().length < 3}>Buscar pessoa</button>
    </form>
    <ul class="people">
      {#each people as p (p.citizen_id)}
        <li>
          <span>{nameOf(p)}</span>
          <button class="btn" disabled={busy} onclick={async () => { if (await run(() => addGroupMember(g.id, p.citizen_id))) { people = []; personQuery = ''; } }}>Adicionar</button>
        </li>
      {/each}
    </ul>
    <div class="row">
      <button class="btn" disabled={busy} onclick={() => run(() => updateGroup(g.id, { entry: g.entry === 'open' ? 'approval' : 'open' }))}>
        {g.entry === 'open' ? 'Exigir aprovação' : 'Abrir a entrada'}
      </button>
      {#if !g.forum_path}
        <button class="btn" disabled={busy} onclick={() => run(() => enableGroupForum(g.id))}>Criar fórum privado</button>
      {/if}
      <button class="btn" disabled={busy} onclick={() => archive(g.id)}>Arquivar grupo</button>
    </div>
  {/if}
{:else}
  <h1>Grupos</h1>
  {#if groups.length === 0}
    <p class="muted">Nenhum grupo criado ainda.</p>
  {:else}
    <ul class="list">
      {#each groups as g (g.id)}
        <li>
          <a href={`/grupos?g=${encodeURIComponent(g.slug)}`}><strong>{g.name}</strong></a>
          {#if g.description}<p class="muted">{g.description}</p>{/if}
          <p class="muted small">
            {g.member_count} {g.member_count === 1 ? 'participante' : 'participantes'}
            {#if g.my_status === 'active'} · você participa{:else if g.my_status === 'pending'} · pedido enviado{/if}
          </p>
        </li>
      {/each}
    </ul>
  {/if}
  {#if isAdmin}
    <details class="new">
      <summary>Criar grupo</summary>
      <form onsubmit={(e) => { e.preventDefault(); void create(); }}>
        <label>Nome <input class="input" type="text" maxlength="120" required bind:value={draft.name} /></label>
        <label>Endereço <input class="input" type="text" maxlength="61" pattern="[a-z0-9][a-z0-9\-]+" required bind:value={draft.slug} /></label>
        <label>Descrição <input class="input" type="text" bind:value={draft.description} /></label>
        <label>Entrada
          <select class="input" bind:value={draft.entry}>
            <option value="approval">Com aprovação da coordenação</option>
            <option value="open">Aberta</option>
          </select>
        </label>
        <button class="btn btn-primary" type="submit" disabled={busy}>Criar</button>
      </form>
    </details>
  {/if}
{/if}

<style>
  .row { display: flex; flex-wrap: wrap; gap: 0.5rem; align-items: center; margin: 0.75rem 0; }
  .people, .list { list-style: none; padding: 0; display: grid; gap: 0.5rem; }
  .people li { display: flex; flex-wrap: wrap; gap: 0.5rem; align-items: center; }
  .people li span { flex: 1 1 12rem; }
  .list li { padding: 0.75rem 0; border-bottom: 1px solid var(--border-subtle); }
  .new form { display: grid; gap: 0.5rem; max-width: 32rem; margin-top: 0.5rem; }
  .new label { display: grid; gap: 0.2rem; }
</style>
