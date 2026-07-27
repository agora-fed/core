<script lang="ts">
  // /admin/papeis (R4) — cria/edita papéis configuráveis com matriz de permissões
  // por categoria (estilo Mastodon). As chaves vêm do catálogo dos manifestos.
  import { onMount } from 'svelte';
  import {
    getPermissionCatalog,
    listRoles,
    createRole,
    updateRole,
    deleteRole,
    listRoleMembers,
    addRoleMember,
    removeRoleMember,
    type PermissionCatalogItem,
    type RoleDto,
    type RoleMemberDto,
    type RoleInput,
  } from '../../lib/api';
  import { toast } from '../../lib/toasts';

  let loading = $state(true);
  let error = $state<string | null>(null);
  let catalog = $state<PermissionCatalogItem[]>([]);
  let roles = $state<RoleDto[]>([]);

  // Editor
  let editing = $state<RoleDto | null>(null);
  let isNew = $state(false);
  let form = $state<RoleInput>({
    name: '',
    color: '#2563eb',
    position: 1,
    permissions: [],
    highlighted: true,
  });
  let busy = $state(false);

  // Membros do papel em edição
  let members = $state<RoleMemberDto[]>([]);
  let newMemberHandle = $state('');

  // Categorias na ordem de exibição.
  const CATEGORY_ORDER = ['special', 'moderation', 'administration', 'invites'];
  let byCategory = $derived(
    CATEGORY_ORDER.map((slug) => ({
      slug,
      label: catalog.find((c) => c.category === slug)?.category_label ?? slug,
      items: catalog.filter((c) => c.category === slug),
    })).filter((g) => g.items.length > 0),
  );

  async function reload() {
    loading = true;
    error = null;
    const [cat, rs] = await Promise.all([getPermissionCatalog(), listRoles()]);
    loading = false;
    if (!cat.success || !rs.success) {
      error =
        cat.error?.message ??
        rs.error?.message ??
        'Não foi possível carregar (precisa da permissão roles.manage).';
      return;
    }
    catalog = cat.data ?? [];
    roles = rs.data ?? [];
  }

  onMount(reload);

  function startNew() {
    isNew = true;
    editing = null;
    form = { name: '', color: '#2563eb', position: 1, permissions: [], highlighted: true };
    members = [];
  }

  async function startEdit(r: RoleDto) {
    isNew = false;
    editing = r;
    form = {
      name: r.name,
      color: r.color ?? '#2563eb',
      position: r.position,
      permissions: [...r.permissions],
      highlighted: r.highlighted,
    };
    const m = await listRoleMembers(r.id);
    members = m.success && m.data ? m.data : [];
  }

  function cancelEdit() {
    editing = null;
    isNew = false;
  }

  function togglePerm(key: string) {
    form.permissions = form.permissions.includes(key)
      ? form.permissions.filter((k) => k !== key)
      : [...form.permissions, key];
  }

  async function save() {
    if (busy || !form.name.trim()) return;
    busy = true;
    const res = isNew ? await createRole(form) : await updateRole(editing!.id, form);
    busy = false;
    if (res.success) {
      toast.success(isNew ? 'Papel criado.' : 'Papel atualizado.');
      cancelEdit();
      await reload();
    } else {
      toast.error(res.error?.message ?? 'Não foi possível salvar.');
    }
  }

  async function remove(r: RoleDto) {
    if (!window.confirm(`Remover o papel "${r.name}"? Os membros perdem essas permissões.`)) return;
    const res = await deleteRole(r.id);
    if (res.success) {
      toast.success('Papel removido.');
      if (editing?.id === r.id) cancelEdit();
      await reload();
    } else {
      toast.error(res.error?.message ?? 'Não foi possível remover.');
    }
  }

  async function addMember() {
    if (!editing || !newMemberHandle.trim()) return;
    const res = await addRoleMember(editing.id, newMemberHandle.trim());
    if (res.success) {
      newMemberHandle = '';
      const m = await listRoleMembers(editing.id);
      members = m.success && m.data ? m.data : [];
    } else {
      toast.error(res.error?.message ?? 'Não foi possível adicionar.');
    }
  }

  async function dropMember(cid: string) {
    if (!editing) return;
    const res = await removeRoleMember(editing.id, cid);
    if (res.success) {
      members = members.filter((m) => m.citizen_id !== cid);
    } else {
      toast.error(res.error?.message ?? 'Não foi possível remover.');
    }
  }
</script>

{#if loading}
  <p class="muted">Carregando…</p>
{:else if error}
  <div class="err">{error}</div>
{:else}
  <div class="layout">
    <section class="list">
      <div class="list-head">
        <h2>Papéis</h2>
        <button type="button" class="btn btn-primary" onclick={startNew}>+ Novo papel</button>
      </div>
      <ul>
        {#each roles as r (r.id)}
          <li>
            <button type="button" class="role-row" class:active={editing?.id === r.id} onclick={() => startEdit(r)}>
              <span class="dot" style={`background:${r.color ?? '#888'}`}></span>
              <span class="rname">{r.name}</span>
              <span class="rpos muted">pos {r.position}</span>
              <span class="rperms muted">{r.permissions.includes('administrator') ? 'acesso total' : `${r.permissions.length} permissões`}</span>
            </button>
          </li>
        {/each}
      </ul>
    </section>

    {#if isNew || editing}
      <section class="editor">
        <h2>{isNew ? 'Novo papel' : `Editar: ${editing?.name}`}</h2>
        <div class="grid">
          <label>Nome<input type="text" bind:value={form.name} maxlength="60" /></label>
          <label>Cor<input type="color" bind:value={form.color} /></label>
          <label>Posição (hierarquia)<input type="number" bind:value={form.position} /></label>
          <label class="chk"><input type="checkbox" bind:checked={form.highlighted} /> Exibir badge no perfil</label>
        </div>

        <h3>Permissões</h3>
        {#each byCategory as group (group.slug)}
          <fieldset>
            <legend>{group.label}</legend>
            <div class="perms">
              {#each group.items as p (p.key)}
                <label class="perm">
                  <input type="checkbox" checked={form.permissions.includes(p.key)} onchange={() => togglePerm(p.key)} />
                  <span>{p.label}</span>
                  <code class="muted">{p.key}</code>
                </label>
              {/each}
            </div>
          </fieldset>
        {/each}

        <div class="actions">
          <button type="button" class="btn btn-primary" onclick={save} disabled={busy || !form.name.trim()}>
            {busy ? 'Salvando…' : 'Salvar'}
          </button>
          <button type="button" class="btn" onclick={cancelEdit}>Cancelar</button>
          {#if !isNew && editing}
            <button type="button" class="btn btn-danger" onclick={() => remove(editing!)}>Remover papel</button>
          {/if}
        </div>

        {#if !isNew && editing}
          <h3>Membros</h3>
          <div class="add-member">
            <input type="text" placeholder="@handle do cidadão" bind:value={newMemberHandle} />
            <button type="button" class="btn" onclick={addMember}>Adicionar</button>
          </div>
          {#if members.length === 0}
            <p class="muted small">Nenhum membro neste papel.</p>
          {:else}
            <ul class="members">
              {#each members as m (m.citizen_id)}
                <li>
                  <span>{m.display_name ?? m.handle ?? m.citizen_id}</span>
                  {#if m.handle}<span class="muted small">@{m.handle}</span>{/if}
                  <button type="button" class="f-linklike" onclick={() => dropMember(m.citizen_id)}>remover</button>
                </li>
              {/each}
            </ul>
          {/if}
        {/if}
      </section>
    {:else}
      <section class="editor empty">
        <p class="muted">Selecione um papel à esquerda ou crie um novo. A matriz de permissões é montada a partir dos módulos ativos.</p>
      </section>
    {/if}
  </div>
{/if}

<style>
  .layout { display: grid; grid-template-columns: 1fr; gap: var(--sp-5); }
  @media (min-width: 900px) { .layout { grid-template-columns: 22rem 1fr; align-items: start; } }
  .list-head { display: flex; align-items: center; justify-content: space-between; margin-bottom: var(--sp-3); }
  .list ul, .members { list-style: none; padding: 0; margin: 0; }
  .role-row { display: flex; align-items: center; gap: var(--sp-2); width: 100%; text-align: left; background: none; border: 1px solid var(--border-subtle); border-radius: var(--r-sm); padding: var(--sp-2) var(--sp-3); margin-bottom: var(--sp-2); cursor: pointer; color: inherit; }
  .role-row.active { border-color: var(--accent); background: var(--accent-soft); }
  .dot { width: 12px; height: 12px; border-radius: 50%; flex: none; }
  .rname { font-weight: var(--fw-semibold); }
  .rpos, .rperms { font-size: var(--fs-xs); margin-left: auto; }
  .rperms { margin-left: var(--sp-2); }
  .editor { border: 1px solid var(--border-subtle); border-radius: var(--r-md); padding: var(--sp-4); }
  .editor.empty { display: grid; place-items: center; min-height: 8rem; }
  .grid { display: grid; grid-template-columns: 1fr 1fr; gap: var(--sp-3); margin-bottom: var(--sp-4); }
  .grid label { display: flex; flex-direction: column; gap: var(--sp-1); font-size: var(--fs-sm); font-weight: var(--fw-semibold); }
  .grid input[type=text], .grid input[type=number] { font: inherit; padding: var(--sp-2); border: 1px solid var(--border-subtle); border-radius: var(--r-sm); background: var(--surface-1); color: var(--text-1); }
  .chk { flex-direction: row !important; align-items: center; gap: var(--sp-2) !important; }
  fieldset { border: 1px solid var(--border-subtle); border-radius: var(--r-sm); margin: 0 0 var(--sp-3); padding: var(--sp-3); }
  legend { font-weight: var(--fw-semibold); padding: 0 var(--sp-2); }
  .perms { display: grid; grid-template-columns: 1fr; gap: var(--sp-2); }
  @media (min-width: 640px) { .perms { grid-template-columns: 1fr 1fr; } }
  .perm { display: flex; align-items: center; gap: var(--sp-2); font-size: var(--fs-sm); }
  .perm code { font-size: var(--fs-xs); }
  .actions { display: flex; gap: var(--sp-2); margin: var(--sp-4) 0; flex-wrap: wrap; }
  .add-member { display: flex; gap: var(--sp-2); margin-bottom: var(--sp-3); }
  .add-member input { flex: 1; font: inherit; padding: var(--sp-2); border: 1px solid var(--border-subtle); border-radius: var(--r-sm); background: var(--surface-1); color: var(--text-1); }
  .members li { display: flex; align-items: center; gap: var(--sp-2); padding: var(--sp-1) 0; }
  .btn-danger { color: var(--danger); border-color: var(--danger); }
  .f-linklike { background: none; border: none; color: var(--danger); cursor: pointer; text-decoration: underline; margin-left: auto; }
  .err { color: var(--danger); padding: var(--sp-3); border: 1px solid var(--danger); border-radius: var(--r-sm); }
  .small { font-size: var(--fs-xs); }
</style>
