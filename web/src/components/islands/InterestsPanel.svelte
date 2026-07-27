<script lang="ts">
  // Interesses do cidadão (áreas ministeriais) — perfil. Consome /interest-areas + /me/interests.
  import { onMount } from 'svelte';
  import {
    getInterestAreas,
    getMyInterests,
    setMyInterests,
    type InterestArea,
  } from '../../lib/api';
  import { toast } from '../../lib/toasts';

  let loading = $state(true);
  let areas = $state<InterestArea[]>([]);
  let selected = $state<Set<string>>(new Set());
  let busy = $state(false);

  onMount(async () => {
    const [a, mine] = await Promise.all([getInterestAreas(), getMyInterests()]);
    loading = false;
    if (a.success) areas = a.data ?? [];
    if (mine.success) selected = new Set(mine.data ?? []);
  });

  function toggle(slug: string) {
    const next = new Set(selected);
    if (next.has(slug)) next.delete(slug);
    else next.add(slug);
    selected = next;
  }

  async function save() {
    busy = true;
    const res = await setMyInterests([...selected]);
    busy = false;
    if (!res.success) return toast.error(res.error?.message ?? 'Não foi possível salvar');
    toast.success('Interesses salvos');
  }
</script>

<div class="int">
  <p class="muted small">
    Marque as áreas que você quer acompanhar — baseadas na estrutura ministerial. Usaremos isso
    para direcionar atualizações e consultas relevantes para você.
  </p>

  {#if loading}
    <p class="muted">Carregando…</p>
  {:else}
    <div class="grid">
      {#each areas as a (a.slug)}
        <label class="chip" class:on={selected.has(a.slug)}>
          <input
            type="checkbox"
            checked={selected.has(a.slug)}
            onchange={() => toggle(a.slug)}
          />
          <span class="nm">{a.name}</span>
          {#if a.ministry}<span class="min">{a.ministry}</span>{/if}
        </label>
      {/each}
    </div>
    <div class="foot">
      <span class="muted small">{selected.size} selecionada(s)</span>
      <button class="btn" onclick={save} disabled={busy}>Salvar interesses</button>
    </div>
  {/if}
</div>

<style>
  .int { max-width: 44rem; }
  .grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(13rem, 1fr)); gap: var(--sp-2); margin: var(--sp-3) 0; }
  .chip { display: grid; grid-template-columns: auto 1fr; align-items: start; gap: 8px; padding: var(--sp-2) var(--sp-3); border: 1px solid var(--border-subtle); border-radius: var(--r-md); background: var(--surface-1); cursor: pointer; }
  .chip.on { border-color: var(--accent); background: var(--surface-2); }
  .chip input { margin-top: 3px; grid-row: 1 / span 2; }
  .nm { font-weight: var(--fw-semibold); color: var(--text-1); }
  .min { font-size: var(--fs-xs); color: var(--text-3); grid-column: 2; }
  .foot { display: flex; align-items: center; justify-content: space-between; gap: var(--sp-3); margin-top: var(--sp-2); }
  .btn { padding: var(--sp-2) var(--sp-5); border-radius: var(--r-sm); border: 1px solid var(--accent); background: var(--accent); color: #fff; font-weight: var(--fw-semibold); font-size: var(--fs-sm); cursor: pointer; }
  .btn:disabled { opacity: 0.6; }
  .muted { color: var(--text-3); }
  .small { font-size: var(--fs-sm); }
</style>
