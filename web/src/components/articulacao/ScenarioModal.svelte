<script lang="ts">
  // A new scenario: blank, a copy of the current one, or from a real roll call.
  import { onMount, untrack } from 'svelte';
  import Modal from '../ui/Modal.svelte';
  import Button from '../ui/Button.svelte';
  import Input from '../ui/Input.svelte';
  import Select from '../ui/Select.svelte';
  import { toast } from '../../lib/toasts';
  import { api, fmtDate, type Project, type RollCall, type Scenario } from '../../lib/articulacao';

  interface Props {
    open: boolean;
    project: Project;
    current: Scenario | null;
    onclose: () => void;
    oncreated: (id: string) => void;
  }
  let { open = $bindable(), project, current, onclose, oncreated }: Props = $props();

  // The form starts from the values at open time; the modal is created anew each time.
  let name = $state(untrack(() => `Cenário ${project.scenarios.length + 1}`));
  let mode = $state<'blank' | 'copy' | 'roll'>('blank');
  let rollCall = $state('');
  let absent = $state(true);
  let rollCalls = $state<RollCall[]>([]);
  let busy = $state(false);

  onMount(async () => {
    const res = await api.rollCalls(project.bill_id);
    if (res.success && res.data) {
      rollCalls = res.data;
      rollCall = res.data[0]?.id ?? '';
    }
  });

  async function create(e: SubmitEvent) {
    e.preventDefault();
    busy = true;
    const body: Parameters<typeof api.createScenario>[1] = { name };
    if (mode === 'copy' && current) body.copy_from = current.id;
    if (mode === 'roll') {
      body.roll_call = rollCall;
      body.absent_if_missing = absent;
    }
    const res = await api.createScenario(project.id, body);
    busy = false;
    if (!res.success || !res.data) {
      toast.error(res.error?.message ?? 'Não foi possível criar o cenário.');
      return;
    }
    oncreated(res.data);
  }
</script>

<Modal bind:open title="Novo cenário" {onclose}>
  <form id="new-scenario" onsubmit={create}>
    <Input label="Nome" bind:value={name} required maxlength={120} />
    <fieldset class="modes">
      <legend>Começar de</legend>
      <label class="art-check"><input type="radio" bind:group={mode} value="blank" /> Em branco (Presidente já posicionado)</label>
      {#if current}
        <label class="art-check"><input type="radio" bind:group={mode} value="copy" /> Cópia de "{current.name}"</label>
      {/if}
      <label class="art-check">
        <input type="radio" bind:group={mode} value="roll" disabled={!rollCalls.length} /> Uma votação nominal real
      </label>
    </fieldset>
    {#if mode === 'roll'}
      <Select
        label="Votação"
        bind:value={rollCall}
        options={rollCalls.map((r) => ({
          value: r.id,
          label: `${fmtDate(r.voted_on)} · ${r.bill_label ?? r.id} · ${r.description.slice(0, 80)}`,
        }))}
      />
      <label class="art-check"><input type="checkbox" bind:checked={absent} /> Quem não votou fica em "Ausente"</label>
    {/if}
    {#if !rollCalls.length}
      <p class="muted small">
        Nenhuma votação nominal sincronizada{project.bill_id ? ' para esta proposição' : ''} ainda.
      </p>
    {/if}
  </form>
  {#snippet footer()}
    <Button variant="ghost" onclick={onclose}>Cancelar</Button>
    <Button type="submit" variant="primary" loading={busy} disabled={!name.trim() || (mode === 'roll' && !rollCall)} onclick={() => (document.getElementById('new-scenario') as HTMLFormElement | null)?.requestSubmit()}>
      Criar cenário
    </Button>
  {/snippet}
</Modal>

<style>
  .modes {
    display: grid;
    gap: var(--sp-2);
    border: 0;
    padding: 0;
    margin: 0 0 var(--sp-4);
  }
  legend {
    font-weight: var(--fw-semibold);
    font-size: var(--fs-sm);
    margin-bottom: var(--sp-2);
  }
  .small {
    font-size: var(--fs-sm);
  }
</style>
