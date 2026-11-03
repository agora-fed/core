<script lang="ts">
  import { onMount } from 'svelte';
  import { apiGet, apiPut, DEFAULT_ORG_ID } from '../../lib/api';
  import {
    moduleFlagKey,
    politicalPackageState,
    toggleableModules,
    type ModuleState,
  } from '../../lib/modules';
  import { toast } from '../../lib/toasts';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';

  let modules = $state<ModuleState[]>([]);
  let loaded = $state(false);
  let loadError = $state<string | null>(null);
  let busy = $state(false);

  const optional = $derived(toggleableModules(modules));
  const pkg = $derived(politicalPackageState(modules));

  async function load() {
    const res = await apiGet<ModuleState[]>(`/api/v1/orgs/${DEFAULT_ORG_ID}/modules`);
    if (res.ok && res.data) {
      modules = res.data;
      loaded = true;
      loadError = null;
    } else {
      loadError = res.error ?? 'Não foi possível carregar os módulos.';
    }
  }

  onMount(load);

  function setFlag(id: string, enabled: boolean) {
    return apiPut(`/api/v1/admin/orgs/${DEFAULT_ORG_ID}/flags/${moduleFlagKey(id)}`, { enabled });
  }

  async function apply(ids: string[], enabled: boolean, done: string) {
    busy = true;
    for (const id of ids) {
      const res = await setFlag(id, enabled);
      if (!res.success) {
        toast.error(res.error?.message ?? `Falha ao alterar o módulo ${id}.`);
        busy = false;
        await load();
        return;
      }
    }
    toast.success(done);
    busy = false;
    await load();
  }
</script>

{#if loadError}
  <Card><p role="alert">{loadError}</p></Card>
{:else if !loaded}
  <p class="muted">Carregando módulos…</p>
{:else}
  <p class="muted note">
    As mudanças chegam ao site em até 30 segundos. Páginas individuais geradas no build, como o
    perfil de cada político, só aparecem depois de um novo build.
  </p>

  <Card>
    <h2>Pacote político</h2>
    <p class="muted">
      Políticos, partidos, eleições, placar e propostas com prazo de resposta dos mandatos.
    </p>
    {#if pkg.installed}
      <p>Instalado. Este pacote inclui módulos travados e não pode ser removido por aqui.</p>
    {:else}
      <Button
        variant="primary"
        disabled={busy}
        onclick={() => apply(pkg.missing, true, 'Pacote político instalado.')}
      >
        Instalar pacote político
      </Button>
    {/if}
  </Card>

  <Card>
    <h2>Módulos opcionais</h2>
    <ul class="mods">
      {#each optional as m (m.id)}
        <li>
          <span>{m.title}</span>
          <Button
            variant={m.active ? 'ghost' : 'primary'}
            disabled={busy}
            onclick={() =>
              apply([m.id], !m.active, m.active ? `${m.title} desativado.` : `${m.title} ativado.`)}
          >
            {m.active ? 'Desativar' : 'Ativar'}
          </Button>
        </li>
      {/each}
    </ul>
  </Card>
{/if}

<style>
  .note {
    margin-bottom: var(--sp-4);
  }
  .mods {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .mods li {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--sp-3);
    padding: var(--sp-2) 0;
    border-top: 1px solid var(--border-subtle);
  }
  h2 {
    margin: 0 0 var(--sp-1);
  }
</style>
