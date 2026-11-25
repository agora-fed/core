<script lang="ts">
  import { onMount } from 'svelte';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';
  import Input from '../ui/Input.svelte';
  import Select from '../ui/Select.svelte';
  import Badge from '../ui/Badge.svelte';
  import EmptyState from '../ui/EmptyState.svelte';
  import Skeleton from '../ui/Skeleton.svelte';
  import Failure from './Failure.svelte';
  import { toast } from '../../lib/toasts';
  import {
    api,
    failureOf,
    href,
    projectBody,
    roleLabel,
    RULE_CHOICES,
    ruleText,
    thresholdHint,
    type Failure as F,
    type RuleChoice,
    type Role,
    type RoomDetail,
  } from '../../lib/articulacao';

  let { id }: { id: string } = $props();

  let room = $state<RoomDetail | null>(null);
  let failure = $state<F | null>(null);
  let handle = $state('');
  let role = $state<Role>('editor');
  let project = $state<{ name: string; rule: RuleChoice; threshold: string; bill: string }>({
    name: '',
    rule: 'auto',
    threshold: '',
    bill: '',
  });
  let busy = $state(false);

  const isOwner = $derived(room?.role === 'owner');
  const canEdit = $derived(room !== null && room.role !== 'viewer');
  const roles = [
    { value: 'editor', label: 'Editor(a) — pode mover e anotar' },
    { value: 'viewer', label: 'Leitor(a) — só vê' },
    { value: 'owner', label: 'Responsável — gerencia a sala' },
  ];

  async function load() {
    failure = null;
    const res = await api.room(id);
    if (res.success && res.data) room = res.data;
    else failure = failureOf(res);
  }
  onMount(load);

  async function act<T>(call: Promise<{ success: boolean; error: { message: string } | null; data: T | null }>, done?: string) {
    busy = true;
    const res = await call;
    busy = false;
    if (!res.success) {
      toast.error(res.error?.message ?? 'Não foi possível concluir.');
      return null;
    }
    if (done) toast.success(done);
    return res;
  }

  async function addMember(e: SubmitEvent) {
    e.preventDefault();
    if (await act(api.addMember(id, handle, role), 'Pessoa adicionada. Ela recebe um aviso.')) {
      handle = '';
      await load();
    }
  }

  async function removeMember(userId: string, name: string) {
    if (!confirm(`Remover ${name} da sala?`)) return;
    if (await act(api.removeMember(id, userId), 'Pessoa removida.')) await load();
  }

  async function createProject(e: SubmitEvent) {
    e.preventDefault();
    const res = await act(
      api.createProject(id, projectBody(project)),
    );
    if (res?.data) window.location.hash = href.project(res.data);
  }

  async function deleteRoom() {
    if (!room || !confirm(`Excluir a sala "${room.name}" e todos os projetos? Esta ação não pode ser desfeita.`)) return;
    if (await act(api.deleteRoom(id), 'Sala excluída.')) window.location.hash = href.rooms();
  }
</script>

<a class="art-back" href={href.rooms()}>← Salas</a>

{#if failure}
  <Failure {failure} retry={load} />
{:else if !room}
  <Skeleton variant="block" height="12rem" />
{:else}
  <header class="art-head">
    <h1>{room.name}</h1>
    <Button href={href.audit(id)} variant="ghost" size="sm">Histórico de mudanças</Button>
    {#if isOwner}
      <Button variant="danger" size="sm" onclick={deleteRoom} disabled={busy}>Excluir sala</Button>
    {/if}
  </header>

  <div class="layout">
    <Card as="section">
      <h2 class="h2">Projetos</h2>
      {#if room.projects.length === 0}
        <EmptyState icon="ballot" title="Nenhum projeto ainda" description="Um projeto é uma proposição ou uma votação que a sala acompanha." />
      {:else}
        <ul class="list">
          {#each room.projects as p (p.id)}
            <li>
              <a class="row-link" href={href.project(p.id)}>
                <span class="strong">{p.name}</span>
                <span class="muted small">{ruleText(p.rule, p.threshold)}{p.bill_id ? ` · proposição ${p.bill_id}` : ''}</span>
              </a>
            </li>
          {/each}
        </ul>
      {/if}
      {#if canEdit}
        <form class="form" onsubmit={createProject}>
          <h3 class="h3">Novo projeto</h3>
          <Input label="Nome" bind:value={project.name} required maxlength={120} placeholder="Ex.: PEC 45 — 1º turno" />
          <Input label="Id da proposição na Câmara" type="number" inputmode="numeric" bind:value={project.bill} hint="Opcional. Filtra as votações reais e define a regra." />
          <div class="two">
            <Select label="Regra de aprovação" bind:value={project.rule} options={[...RULE_CHOICES]} />
            <Input
              label={project.rule === 'simple' ? 'Quórum' : 'Mínimo para aprovar'}
              type="number"
              inputmode="numeric"
              bind:value={project.threshold}
              hint={thresholdHint(project.rule)}
            />
          </div>
          <Button type="submit" variant="primary" loading={busy} disabled={!project.name.trim()}>Criar projeto</Button>
        </form>
      {/if}
    </Card>

    <Card as="section">
      <h2 class="h2">Pessoas <span class="muted small">({room.members.length})</span></h2>
      <ul class="list">
        {#each room.members as m (m.user_id)}
          <li class="member">
            <span class="member-name">{m.display_name}</span>
            <Badge tone={m.role === 'owner' ? 'accent' : 'neutral'} size="sm">{roleLabel(m.role)}</Badge>
            {#if isOwner}
              <Button variant="ghost" size="sm" ariaLabel={`Remover ${m.display_name}`} onclick={() => removeMember(m.user_id, m.display_name)} disabled={busy}>Remover</Button>
            {/if}
          </li>
        {/each}
      </ul>
      {#if isOwner}
        <form class="form" onsubmit={addMember}>
          <h3 class="h3">Adicionar pessoa</h3>
          <Input label="@ da pessoa" bind:value={handle} required maxlength={100} placeholder="@usuario" autocomplete="off" />
          <Select label="Papel" bind:value={role} options={roles} />
          <Button type="submit" variant="secondary" loading={busy} disabled={!handle.trim()}>Adicionar</Button>
          <p class="muted small note">A pessoa recebe um aviso no site e por e-mail.</p>
        </form>
      {/if}
    </Card>
  </div>
{/if}

<style>
  .layout {
    display: grid;
    gap: var(--sp-5);
    grid-template-columns: minmax(0, 3fr) minmax(0, 2fr);
    align-items: start;
  }
  @media (max-width: 860px) {
    .layout {
      grid-template-columns: minmax(0, 1fr);
    }
  }
  .h2 {
    font-size: var(--fs-lg);
    margin: 0 0 var(--sp-3);
  }
  .h3 {
    font-size: var(--fs-base);
    margin: 0 0 var(--sp-3);
  }
  .small {
    font-size: var(--fs-sm);
  }
  .strong {
    font-weight: var(--fw-semibold);
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .list li {
    border-bottom: 1px solid var(--border-subtle);
  }
  .list li:last-child {
    border-bottom: 0;
  }
  .row-link {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: var(--sp-3) 0;
    color: var(--text-1);
    text-decoration: none;
    overflow-wrap: anywhere;
  }
  .row-link:hover .strong {
    color: var(--accent);
  }
  .member {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-2) 0;
  }
  .member-name {
    flex: 1 1 8rem;
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .form {
    margin-top: var(--sp-5);
    padding-top: var(--sp-4);
    border-top: 1px solid var(--border-subtle);
  }
  .two {
    display: grid;
    gap: var(--sp-3);
    grid-template-columns: repeat(auto-fit, minmax(min(100%, 12rem), 1fr));
  }
  .note {
    margin: var(--sp-2) 0 0;
  }
</style>
