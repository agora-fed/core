<script lang="ts">
  import { untrack } from 'svelte';
  import Modal from '../ui/Modal.svelte';
  import Button from '../ui/Button.svelte';
  import Input from '../ui/Input.svelte';
  import Select from '../ui/Select.svelte';
  import { toast } from '../../lib/toasts';
  import { api, defaultThreshold, projectBody, RULE_CHOICES, thresholdHint, type Project, type RuleChoice } from '../../lib/articulacao';

  interface Props {
    open: boolean;
    project: Project;
    onclose: () => void;
    onsaved: () => void;
    ondeleted: () => void;
  }
  let { open = $bindable(), project, onclose, onsaved, ondeleted }: Props = $props();

  // The form starts from the values at open time; the modal is created anew each time.
  const initial = untrack(() => project);
  let name = $state(initial.name);
  let rule = $state<RuleChoice>(initial.rule ?? 'fixed');
  let threshold = $state(String(initial.threshold));
  let bill = $state(initial.bill_id ? String(initial.bill_id) : '');
  let busy = $state(false);

  async function save(e: SubmitEvent) {
    e.preventDefault();
    busy = true;
    const res = await api.patchProject(project.id, projectBody({ name, rule, threshold, bill }));
    busy = false;
    if (!res.success) {
      toast.error(res.error?.message ?? 'Não foi possível salvar.');
      return;
    }
    toast.success('Projeto salvo.');
    onsaved();
  }

  async function remove() {
    if (!confirm('Excluir o projeto e todos os cenários?')) return;
    const res = await api.deleteProject(project.id);
    if (!res.success) {
      toast.error(res.error?.message ?? 'Não foi possível excluir.');
      return;
    }
    ondeleted();
  }
</script>

<Modal bind:open title="Configurar projeto" {onclose}>
  <form id="project-settings" onsubmit={save}>
    <Input label="Nome" bind:value={name} required maxlength={120} />
    <Select
      label="Regra de aprovação"
      bind:value={rule}
      options={RULE_CHOICES.filter((c) => c.value !== 'auto').map((c) => ({ ...c }))}
      onchange={(v) => (threshold = defaultThreshold(v as RuleChoice, threshold))}
    />
    <Input
      label={rule === 'simple' ? 'Quórum' : 'Mínimo para aprovar'}
      type="number"
      inputmode="numeric"
      bind:value={threshold}
      hint={thresholdHint(rule)}
    />
    <Input label="Id da proposição na Câmara" type="number" inputmode="numeric" bind:value={bill} hint="Opcional." />
  </form>
  {#snippet footer()}
    {#if project.role === 'owner'}
      <Button variant="danger" onclick={remove}>Excluir projeto</Button>
    {/if}
    <span class="spacer"></span>
    <Button variant="ghost" onclick={onclose}>Cancelar</Button>
    <Button variant="primary" loading={busy} disabled={!name.trim()} onclick={() => (document.getElementById('project-settings') as HTMLFormElement | null)?.requestSubmit()}>Salvar</Button>
  {/snippet}
</Modal>

<style>
  .spacer {
    flex: 1;
  }
</style>
