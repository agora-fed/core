<script lang="ts">
  // Public page of ONE party chapter (directory): identity header, scoped
  // administrators (privacy-safe) and the derived members (party mandates in
  // the chapter's territory). Loaded by `?id=` — the SSG runtime-entity
  // pattern (see chapterUrl in lib/parties.ts).
  import { onMount } from 'svelte';
  import {
    getChapter,
    getDirectoryMembers,
    type ChapterDto,
    type DirectoryMemberDto,
  } from '../../lib/api';
  import { partySlug } from '../../lib/parties';

  let { sigla }: { sigla: string } = $props();

  let loading = $state(true);
  let error = $state<string | null>(null);
  let chapter = $state<ChapterDto | null>(null);
  let members = $state<DirectoryMemberDto[] | null>(null);

  const LEVEL_LABEL: Record<string, string> = {
    national: 'Nacional',
    state: 'Estadual',
    municipal: 'Municipal',
  };

  function territoryLabel(c: ChapterDto): string {
    if (c.level === 'municipal') return `${c.municipality} · ${c.state}`;
    if (c.level === 'state') return c.state ?? '';
    return 'Brasil';
  }

  onMount(async () => {
    const id = new URLSearchParams(window.location.search).get('id');
    if (!id) {
      error = 'Diretório não informado.';
      loading = false;
      return;
    }
    const res = await getChapter(sigla, id);
    if (res.ok && res.data) {
      chapter = res.data;
      const mem = await getDirectoryMembers(sigla, id);
      members = mem.ok && mem.data ? mem.data : [];
    } else {
      error = 'Diretório não encontrado.';
    }
    loading = false;
  });
</script>

{#if loading}
  <p class="muted">Carregando…</p>
{:else if error || !chapter}
  <p class="error" role="alert">{error ?? 'Diretório não encontrado.'}</p>
  <p><a href={`/partidos/${partySlug(sigla)}`}>← Voltar ao partido</a></p>
{:else}
  <header class="chapter-head card">
    {#if chapter.party_logo_url}
      <img class="crest" src={chapter.party_logo_url} alt="" loading="lazy" />
    {/if}
    <div>
      <p class="crumbs">
        <a href="/partidos">Partidos</a> ·
        <a href={`/partidos/${partySlug(chapter.party_short_name)}`}>{chapter.party_short_name}</a>
      </p>
      <h1>{chapter.name}</h1>
      <p class="badges">
        <span class="badge">{LEVEL_LABEL[chapter.level] ?? chapter.level}</span>
        <span class="badge badge-soft">{territoryLabel(chapter)}</span>
      </p>
    </div>
  </header>

  <section class="block">
    <h2>Responsáveis</h2>
    {#if chapter.administrators.length === 0}
      <p class="muted">Nenhum responsável público neste diretório.</p>
    {:else}
      <ul class="admins">
        {#each chapter.administrators as a (a.public_handle ?? a.display_name)}
          <li class="card admin">
            <span class="who">
              <strong>{a.display_name ?? a.public_handle ?? 'Sem nome público'}</strong>
              {#if a.public_handle}
                <a class="handle" href={`/perfil/?u=${a.public_handle}`}>@{a.public_handle}</a>
              {/if}
            </span>
            <span class="badge">{a.role}</span>
          </li>
        {/each}
      </ul>
    {/if}
  </section>

  <section class="block">
    <h2>Mandatos no território</h2>
    {#if !members}
      <p class="muted">Carregando…</p>
    {:else if members.length === 0}
      <p class="muted">Nenhum mandato do {chapter.party_short_name} neste território ainda.</p>
    {:else}
      <ul class="grid">
        {#each members as m (m.mandate_id)}
          <li class="card member">
            <a class="link" href={`/politicos/?id=${m.mandate_id}`}>
              {#if m.avatar_url}
                <img class="avatar" src={m.avatar_url} alt="" loading="lazy" />
              {:else}
                <span class="avatar avatar-placeholder">👤</span>
              {/if}
              <div class="meta">
                <strong>{m.display_name}</strong>
                <span class="muted">{m.office}{m.municipio ? ` · ${m.municipio}` : ''}</span>
              </div>
            </a>
          </li>
        {/each}
      </ul>
    {/if}
  </section>
{/if}

<style>
  .chapter-head {
    display: flex;
    gap: var(--sp-4);
    align-items: center;
    padding: var(--sp-5);
    margin-bottom: var(--sp-5);
  }
  .crest {
    width: 64px;
    height: 64px;
    object-fit: contain;
  }
  .crumbs {
    margin: 0 0 var(--sp-1);
    font-size: var(--fs-sm);
  }
  .crumbs a {
    color: var(--text-2);
    text-decoration: none;
  }
  h1 {
    margin: 0 0 var(--sp-2);
    font-size: var(--fs-2xl);
  }
  .badges {
    display: flex;
    gap: var(--sp-2);
    margin: 0;
  }
  .badge {
    background: var(--accent-soft);
    color: var(--accent-strong);
    border-radius: 999px;
    padding: 0.15rem 0.7rem;
    font-size: var(--fs-xs);
    font-weight: 600;
  }
  .badge-soft {
    background: var(--surface-2);
    color: var(--text-2);
  }
  .block {
    margin-bottom: var(--sp-6);
  }
  .block h2 {
    font-size: var(--fs-lg);
    margin: 0 0 var(--sp-3);
  }
  .admins,
  .grid {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: var(--sp-2);
  }
  .grid {
    grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
  }
  .admin {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: var(--sp-3);
  }
  .who {
    display: flex;
    gap: var(--sp-2);
    align-items: baseline;
  }
  .handle {
    font-size: var(--fs-sm);
  }
  .member .link {
    display: flex;
    gap: var(--sp-3);
    align-items: center;
    padding: var(--sp-3);
    text-decoration: none;
    color: inherit;
  }
  .avatar {
    width: 40px;
    height: 40px;
    border-radius: 50%;
    object-fit: cover;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: var(--surface-2);
  }
  .meta {
    display: flex;
    flex-direction: column;
  }
  .muted {
    color: var(--text-3);
  }
  .error {
    color: var(--danger);
  }
</style>
