<script lang="ts">
  // Página de agregação territorial (Fase 2.2): "meu município".
  // /municipio/?uf=RJ&m=RIO DE JANEIRO — SSG não conhece os 5.5k municípios,
  // a ilha resolve no client (mesmo padrão de /campanha e /perfil). Mostra o
  // eleitorado oficial e os mandatos municipais agrupados por partido.
  import { onMount } from 'svelte';
  import {
    getTerritorio,
    browsePoliticos,
    type TerritorioResponse,
    type PoliticoRow,
  } from '../../lib/api';
  import { partyColor } from '../../lib/parties';

  let loading = $state(true);
  let error = $state<string | null>(null);
  let uf = $state('');
  let municipio = $state('');
  let summary = $state<TerritorioResponse | null>(null);
  let byParty = $state<Record<string, PoliticoRow[]>>({});
  let partyOrder = $state<string[]>([]);

  const fmt = new Intl.NumberFormat('pt-BR');

  onMount(async () => {
    const params = new URLSearchParams(window.location.search);
    uf = (params.get('uf') ?? '').toUpperCase();
    municipio = params.get('m') ?? params.get('municipio') ?? '';
    if (uf.length !== 2 || !municipio) {
      error = 'Município não informado. Volte à busca de políticos e escolha um município.';
      loading = false;
      return;
    }

    const [sumRes, browseRes] = await Promise.all([
      getTerritorio(uf, municipio),
      browsePoliticos({ sphere: 'municipal', uf, municipio, limit: 1000 }),
    ]);
    loading = false;

    // getTerritorio/browsePoliticos passam por fetchedToApiResponse → shape
    // { success, data, error:{message} } (não o { ok } do apiGet cru).
    if (sumRes.success && sumRes.data) summary = sumRes.data;

    if (browseRes.success && browseRes.data) {
      const groups: Record<string, PoliticoRow[]> = {};
      for (const m of browseRes.data.items) {
        const key = m.party ?? '—';
        (groups[key] ??= []).push(m);
      }
      for (const k of Object.keys(groups)) {
        groups[k].sort((a, b) => a.display_name.localeCompare(b.display_name, 'pt-BR'));
      }
      byParty = groups;
      // Ordem: partidos com mais mandatos primeiro (segue o resumo se veio).
      partyOrder =
        summary?.by_party.map((p) => p.party).filter((p) => groups[p]) ??
        Object.keys(groups).sort((a, b) => groups[b].length - groups[a].length);
      // Inclui partidos que apareceram no browse mas não no resumo (defensivo).
      for (const k of Object.keys(groups)) if (!partyOrder.includes(k)) partyOrder.push(k);
    } else if (!summary) {
      error = browseRes.error?.message ?? 'Não foi possível carregar o município.';
    }
  });

  function officeShort(o: string): string {
    // O office do seed às vezes embute a cidade ("Vereador(a) — RIO DE JANEIRO/RJ").
    // Corta no travessão pra mostrar só o cargo.
    return o.split(' — ')[0].split(' - ')[0].trim();
  }
</script>

{#if loading}
  <p class="muted">Carregando município…</p>
{:else if error}
  <div class="card center">
    <p class="hint-error" role="alert">{error}</p>
    <p class="muted"><a href="/politicos">Ir para a busca de políticos</a></p>
  </div>
{:else}
  <header class="head">
    <div>
      <p class="eyebrow muted">Município · {uf}</p>
      <h1>{municipio}</h1>
    </div>
  </header>

  <div class="stats">
    <div class="stat card">
      <span class="stat-val">{summary?.voters != null ? fmt.format(summary.voters) : '—'}</span>
      <span class="stat-lbl muted">eleitores (TSE)</span>
    </div>
    <div class="stat card">
      <span class="stat-val">{summary?.total ?? 0}</span>
      <span class="stat-lbl muted">mandatos municipais</span>
    </div>
    <div class="stat card">
      <span class="stat-val">{partyOrder.length}</span>
      <span class="stat-lbl muted">partidos representados</span>
    </div>
  </div>

  {#if partyOrder.length === 0}
    <div class="card center">
      <p>Nenhum mandato municipal cadastrado para {municipio}/{uf} ainda.</p>
    </div>
  {:else}
    {#each partyOrder as sigla (sigla)}
      {@const group = byParty[sigla]}
      <section class="party-group" style={`--party-accent:${partyColor(sigla)}`}>
        <div class="pg-head">
          <a class="pg-crest" href={`/partidos/${encodeURIComponent(sigla.toLowerCase())}`}>
            {sigla.slice(0, 4)}
          </a>
          <h2>{sigla}</h2>
          <span class="muted count">{group.length}</span>
        </div>
        <ul class="grid">
          {#each group as m (m.id)}
            <li class="card member">
              <a class="link" href={`/politicos/${m.id}`}>
                {#if m.avatar_url}
                  <img class="avatar" src={m.avatar_url} alt="" loading="lazy" />
                {:else}
                  <span class="avatar avatar-placeholder">👤</span>
                {/if}
                <div class="meta">
                  <strong class="name">
                    {m.display_name}
                    {#if m.has_verified_operator}
                      <span class="badge-verified" title="Vínculo verificado" aria-label="Vínculo verificado">✓</span>
                    {/if}
                  </strong>
                  <span class="muted office">{officeShort(m.office)}</span>
                </div>
              </a>
            </li>
          {/each}
        </ul>
      </section>
    {/each}
  {/if}
{/if}

<style>
  .head { margin-bottom: 1.5rem; }
  .eyebrow { text-transform: uppercase; letter-spacing: 0.05em; font-size: 0.8rem; margin: 0 0 0.2rem; }
  .head h1 { margin: 0; font-size: 1.8rem; }

  .stats {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(140px, 1fr));
    gap: 0.8rem;
    margin-bottom: 2rem;
  }
  .stat { padding: 1rem; display: grid; gap: 0.2rem; text-align: center; }
  .stat-val { font-size: 1.6rem; font-weight: 800; font-variant-numeric: tabular-nums; color: var(--text-1, inherit); }
  .stat-lbl { font-size: 0.8rem; }

  .party-group { margin-bottom: 2rem; }
  .pg-head { display: flex; align-items: center; gap: 0.7rem; margin-bottom: 0.8rem; }
  .pg-crest {
    width: 40px; height: 40px; border-radius: 9px;
    background: var(--party-accent, #888); color: #fff;
    display: inline-flex; align-items: center; justify-content: center;
    font-weight: 700; font-size: 0.8rem; text-decoration: none; flex-shrink: 0;
  }
  .pg-head h2 { margin: 0; font-size: 1.2rem; color: var(--text-1, inherit); }
  .count { margin-left: auto; font-variant-numeric: tabular-nums; }

  .grid {
    list-style: none; padding: 0; margin: 0;
    display: grid; gap: 0.7rem;
    grid-template-columns: repeat(auto-fill, minmax(230px, 1fr));
  }
  .card {
    background: var(--surface-1, #fff);
    border: 1px solid var(--border-subtle, rgba(0,0,0,0.1));
    border-radius: 12px;
    overflow: hidden;
  }
  .member { transition: transform 100ms ease, box-shadow 100ms ease; }
  .member:hover { transform: translateY(-2px); box-shadow: 0 6px 16px rgba(0,0,0,0.07); }
  .link { display: flex; gap: 0.8rem; padding: 0.85rem; text-decoration: none; color: inherit; align-items: center; }
  .avatar { width: 52px; height: 52px; border-radius: 50%; object-fit: cover; background: var(--surface-2, #eee); flex-shrink: 0; }
  .avatar-placeholder { display: inline-flex; align-items: center; justify-content: center; font-size: 1.3rem; }
  .meta { display: grid; gap: 0.15rem; min-width: 0; }
  .name { font-size: 0.98rem; line-height: 1.2; }
  .office { font-size: 0.83rem; }
  .center { text-align: center; padding: 2.5rem 1.5rem; }
  .hint-error { color: #dc2626; }
  .badge-verified {
    display: inline-flex; align-items: center; justify-content: center;
    background: var(--c-green-soft, #d4edda); color: var(--c-green-dark, #1e6b3a);
    font-weight: 700; border-radius: 999px; line-height: 1;
    padding: 0.1rem 0.35rem; margin-left: 0.3rem; font-size: 0.7rem; vertical-align: middle;
  }
</style>
