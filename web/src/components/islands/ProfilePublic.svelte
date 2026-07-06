<script lang="ts">
  // Perfil público humano (cidadão OU político). CSR: como o site é SSG (ADR-0009) e não dá pra
  // pré-gerar um HTML por handle arbitrário no build, esta ilha lê o handle do query-param `?u=`
  // (client-side) e hidrata o perfil via GET /api/v1/profiles/{handle}. É o alvo do 302 que o
  // gateway emite quando um navegador acessa /actors/{handle}.
  import { onMount } from 'svelte';
  import { getPublicProfile, DEFAULT_ORG_ID } from '../../lib/api';
  import type { ProfileDto } from '../../lib/types';

  let { handle: handleProp = '' }: { handle?: string } = $props();

  let handle = $state(handleProp);
  let loading = $state(true);
  let profile = $state<ProfileDto | null>(null);
  let loadError = $state<string | null>(null);

  let verifBadge = $derived.by(() => {
    const lvl = profile?.verification_level ?? 'none';
    switch (lvl) {
      case 'directory':
        return { label: 'Vínculo verificado', cls: 'ok' };
      case 'cpf':
        return { label: 'CPF verificado', cls: 'ok' };
      default:
        return null;
    }
  });

  onMount(async () => {
    if (!handle) {
      handle = new URLSearchParams(window.location.search).get('u')?.trim() ?? '';
    }
    if (!handle) {
      loading = false;
      loadError = 'Perfil não informado.';
      return;
    }
    const res = await getPublicProfile(handle, DEFAULT_ORG_ID);
    loading = false;
    if (!res.ok || !res.data) {
      loadError = res.error?.includes('not found')
        ? 'Este perfil não existe ou não é público.'
        : (res.error ?? 'Não foi possível carregar o perfil.');
      return;
    }
    profile = res.data;
  });
</script>

{#if loading}
  <p class="muted">Carregando…</p>
{:else if loadError}
  <div class="card center" role="alert">
    <h2>{loadError}</h2>
    <p class="muted"><a href="/">Voltar para o início</a></p>
  </div>
{:else if profile}
  <article class="profile">
    <div class="cover" style={profile.cover_url ? `background-image:url(${profile.cover_url})` : ''}></div>
    <header class="head">
      {#if profile.avatar_url}
        <img class="avatar-lg" src={profile.avatar_url} alt="" />
      {:else}
        <span class="avatar-lg avatar-placeholder">👤</span>
      {/if}
      <div class="head-meta">
        <h1>{profile.display_name ?? profile.handle ?? 'Cidadã(o)'}</h1>
        <p class="handle">@{profile.handle ?? profile.public_handle}</p>
        {#if verifBadge}
          <span class="badge badge-{verifBadge.cls}">✓ {verifBadge.label}</span>
        {/if}
      </div>
    </header>

    {#if profile.bio}
      <section class="bio"><p>{profile.bio}</p></section>
    {/if}
  </article>
{/if}

<style>
  .profile {
    border: 1px solid var(--c-border);
    border-radius: 14px;
    overflow: hidden;
    background: var(--c-surface, #fff);
  }
  .cover {
    height: 160px;
    background: linear-gradient(120deg, var(--c-navy, #123), var(--c-green-dark, #1a6));
    background-size: cover;
    background-position: center;
  }
  .head {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 1.25rem;
    align-items: flex-end;
    padding: 0 1.5rem 1.25rem;
    margin-top: -48px;
  }
  .avatar-lg {
    width: 112px;
    height: 112px;
    border-radius: 50%;
    object-fit: cover;
    background: var(--c-bg);
    border: 4px solid var(--c-surface, #fff);
  }
  .avatar-placeholder {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    font-size: 3rem;
  }
  .head-meta {
    padding-bottom: 0.4rem;
  }
  .head h1 {
    margin: 0 0 0.15rem;
    font-size: 1.5rem;
  }
  .handle {
    margin: 0 0 0.5rem;
    color: var(--c-text-muted);
    font-size: 0.95rem;
  }
  .badge-ok {
    background: #e6f7ed;
    color: var(--c-green-dark, #1a6);
    border: 1px solid #b7e4c7;
    border-radius: 999px;
    padding: 0.15rem 0.6rem;
    font-size: 0.82rem;
    font-weight: 600;
  }
  .bio {
    padding: 0 1.5rem 1.5rem;
  }
  .bio p {
    margin: 0;
    white-space: pre-wrap;
    line-height: 1.55;
  }
  .center {
    text-align: center;
    padding: 2.5rem 1.5rem;
  }
  @media (max-width: 640px) {
    .head {
      grid-template-columns: auto 1fr;
    }
    .avatar-lg {
      width: 88px;
      height: 88px;
    }
  }
</style>
