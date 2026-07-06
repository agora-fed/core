<script lang="ts">
  // Detalhe de um partido: seus representantes agrupados por ESFERA (federal/estadual/municipal) —
  // que é a visualização do conceito de "diretório/subgrupo" antes de a fatia B formalizá-lo em
  // tabela. Dentro do federal, subdivide por casa (Câmara/Senado). CSR filtrando getMandates pela
  // sigla recebida via prop (o Astro resolve o slug->sigla no build).
  import { onMount } from 'svelte';
  import { getAllMandates, DEFAULT_ORG_ID, type MandateDto } from '../../lib/api';
  import { partyColor } from '../../lib/parties';

  let { sigla }: { sigla: string } = $props();

  let loading = $state(true);
  let members = $state<MandateDto[]>([]);
  let loadError = $state<string | null>(null);

  const SPHERES: Array<{ key: 'federal' | 'estadual' | 'municipal'; label: string; hint: string }> = [
    { key: 'federal', label: 'Diretório Federal', hint: 'Câmara dos Deputados + Senado' },
    { key: 'estadual', label: 'Diretórios Estaduais', hint: 'Assembleias legislativas + governos' },
    { key: 'municipal', label: 'Diretórios Municipais', hint: 'Câmaras municipais + prefeituras' },
  ];

  let bySphere = $derived.by(() => {
    const groups: Record<string, MandateDto[]> = { federal: [], estadual: [], municipal: [] };
    for (const m of members) groups[m.sphere ?? 'federal'].push(m);
    for (const k of Object.keys(groups)) {
      groups[k].sort((a, b) => a.display_name.localeCompare(b.display_name, 'pt-BR'));
    }
    return groups;
  });

  let accent = $derived(partyColor(sigla));

  onMount(async () => {
    const res = await getAllMandates(DEFAULT_ORG_ID);
    loading = false;
    if (res.ok && res.data) {
      members = res.data.filter((m) => m.party === sigla);
    } else {
      loadError = res.error ?? 'Não foi possível carregar o partido.';
    }
  });

  function houseLabel(m: MandateDto): string {
    if (m.house === 'camara') return 'Câmara';
    if (m.house === 'senado') return 'Senado';
    return m.office;
  }
</script>

<header class="head" style={`--party-accent:${accent}`}>
  <span class="crest" aria-hidden="true">{sigla.slice(0, 4)}</span>
  <div>
    <h1>{sigla}</h1>
    {#if !loading}
      <p class="muted">{members.length} representante{members.length === 1 ? '' : 's'} na plataforma</p>
    {/if}
  </div>
</header>

{#if loading}
  <p class="muted">Carregando…</p>
{:else if loadError}
  <p class="hint hint-error" role="alert">{loadError}</p>
{:else if members.length === 0}
  <div class="card center">
    <p>Nenhum representante deste partido carregado ainda.</p>
    <p class="muted"><a href="/partidos">Voltar aos partidos</a></p>
  </div>
{:else}
  {#each SPHERES as s (s.key)}
    {@const group = bySphere[s.key]}
    {#if group.length > 0}
      <section class="directory">
        <div class="dir-head">
          <h2>{s.label}</h2>
          <span class="muted count">{group.length}</span>
        </div>
        <p class="dir-hint muted">{s.hint}</p>
        <ul class="grid">
          {#each group as m (m.id)}
            <li class="card">
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
                      <span
                        class="badge-verified badge-verified-small"
                        title="Mandato com operador verificado"
                        aria-label="Vínculo verificado"
                      >✓</span>
                    {/if}
                  </strong>
                  <span class="muted office">{houseLabel(m)}{m.uf ? ` · ${m.uf}` : ''}</span>
                </div>
              </a>
            </li>
          {/each}
        </ul>
      </section>
    {/if}
  {/each}
{/if}

<style>
  .head {
    display: flex;
    align-items: center;
    gap: 1rem;
    padding-bottom: 1.5rem;
    margin-bottom: 1.5rem;
    border-bottom: 1px solid var(--c-border);
  }
  .crest {
    width: 64px;
    height: 64px;
    border-radius: 12px;
    background: var(--party-accent, var(--c-bg));
    color: #fff;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    font-weight: 700;
    flex-shrink: 0;
    letter-spacing: -0.02em;
  }
  .head h1 { margin: 0 0 0.2rem; font-size: 1.6rem; }
  .head p { margin: 0; }
  .directory { margin-bottom: 2rem; }
  .dir-head { display: flex; align-items: baseline; gap: 0.5rem; }
  .dir-head h2 { margin: 0; font-size: 1.1rem; color: var(--c-navy); }
  .count { font-variant-numeric: tabular-nums; }
  .dir-hint { margin: 0.15rem 0 0.8rem; font-size: 0.85rem; }
  .grid {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: 0.7rem;
    grid-template-columns: repeat(auto-fill, minmax(230px, 1fr));
  }
  .card {
    background: var(--c-paper);
    border: 1px solid var(--c-border);
    border-radius: 12px;
    overflow: hidden;
    transition: transform 100ms ease, box-shadow 100ms ease;
  }
  .card:hover { transform: translateY(-2px); box-shadow: 0 6px 16px rgba(0,0,0,0.07); }
  .link { display: flex; gap: 0.8rem; padding: 0.85rem; text-decoration: none; color: inherit; align-items: center; }
  .avatar { width: 56px; height: 56px; border-radius: 50%; object-fit: cover; background: var(--c-bg); flex-shrink: 0; }
  .avatar-placeholder { display: inline-flex; align-items: center; justify-content: center; font-size: 1.4rem; }
  .meta { display: grid; gap: 0.15rem; min-width: 0; }
  .name { font-size: 1rem; line-height: 1.2; }
  .office { font-size: 0.85rem; }
  .center { text-align: center; padding: 2.5rem 1.5rem; }
  /* Verified-operator badge: same shape used on the /politicos grid. Subtle green pill so the
     eye reads "positivo" without competing with the party crest colour above. */
  .badge-verified {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: var(--c-green-soft, #d4edda);
    color: var(--c-green-dark, #1e6b3a);
    font-weight: 700;
    border-radius: 999px;
    line-height: 1;
    padding: 0.15rem 0.45rem;
    margin-left: 0.35rem;
    vertical-align: middle;
  }
  .badge-verified-small {
    font-size: 0.7rem;
    padding: 0.1rem 0.35rem;
  }
</style>
