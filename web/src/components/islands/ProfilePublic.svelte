<script lang="ts">
  // Perfil público humano (cidadão OU político). CSR: como o site é SSG (ADR-0009) e não dá pra
  // pré-gerar um HTML por handle arbitrário no build, esta ilha lê o handle do query-param `?u=`
  // (client-side) e hidrata o perfil via GET /api/v1/profiles/{handle}. É o alvo do 302 que o
  // gateway emite quando um navegador acessa /actors/{handle}.
  import { onMount } from 'svelte';
  import { getPublicProfile, DEFAULT_ORG_ID } from '../../lib/api';
  import type { ProfileDto } from '../../lib/types';
  import { formatDate } from '../../lib/format';

  let { handle: handleProp = '' }: { handle?: string } = $props();

  let handle = $state(handleProp);
  let loading = $state(true);
  let profile = $state<ProfileDto | null>(null);
  let loadError = $state<string | null>(null);
  // Endereço federado (@handle@host) — só faz sentido pra perfis públicos com @ escolhido.
  let fediAddress = $state<string | null>(null);
  let copied = $state(false);

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

  // Badge de cidadania política (0.25.0-fediverso): sinaliza pra outros que a
  // conta é de brasileira(o) apta a votar em pauta urgente. Não expõe o número
  // do título — só o status.
  let tituloBadge = $derived.by(() => {
    switch (profile?.titulo_status) {
      case 'verified':
        return { label: 'Cidadania política verificada (TSE)', cls: 'ok' };
      case 'validated':
        return { label: 'Título de eleitor validado', cls: 'ok' };
      default:
        return null;
    }
  });

  let displayName = $derived(
    profile?.display_name ?? profile?.handle ?? 'Cidadã(o)',
  );
  let initials = $derived((displayName.charAt(0) || '?').toUpperCase());

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
    if (profile.is_public && profile.handle) {
      fediAddress = `@${profile.handle}@${window.location.host}`;
    }
  });

  async function copyFedi() {
    if (!fediAddress) return;
    try {
      await navigator.clipboard.writeText(fediAddress);
      copied = true;
      setTimeout(() => (copied = false), 2000);
    } catch {
      /* clipboard bloqueado — o endereço continua visível pra copiar na mão */
    }
  }
</script>

{#if loading}
  <div class="profile sk" aria-label="Carregando perfil…">
    <div class="cover sk-cover"></div>
    <div class="head">
      <span class="avatar-lg sk-block sk-circle"></span>
      <div class="head-meta">
        <span class="sk-line w50"></span>
        <span class="sk-line w30"></span>
      </div>
    </div>
  </div>
{:else if loadError}
  <div class="card state" role="alert">
    <h2>{loadError}</h2>
    <p class="muted">
      O perfil pode ser privado — na DemocraciaBR todo perfil nasce privado e
      só aparece aqui se a pessoa o tornar público.
    </p>
    <a class="btn btn-ghost" href="/">Voltar para o início</a>
  </div>
{:else if profile}
  <article class="profile">
    <div
      class="cover"
      style={profile.cover_url ? `background-image:url(${profile.cover_url})` : ''}
    ></div>
    <header class="head">
      {#if profile.avatar_url}
        <img class="avatar-lg" src={profile.avatar_url} alt="" />
      {:else}
        <span class="avatar-lg avatar-fallback" aria-hidden="true">{initials}</span>
      {/if}
      <div class="head-meta">
        <h1>{displayName}</h1>
        <p class="handle">@{profile.handle ?? profile.public_handle}</p>
        <div class="chips">
          {#if verifBadge}
            <span class="chip chip-ok">✓ {verifBadge.label}</span>
          {/if}
          {#if tituloBadge}
            <span
              class="chip chip-titulo"
              title="Cidadã(o) com título de eleitor validado — vota em pauta urgente."
            >
              🇧🇷 {tituloBadge.label}
            </span>
          {/if}
          {#if profile.created_at}
            <span class="chip chip-plain" title={formatDate(profile.created_at)}>
              Por aqui desde {formatDate(profile.created_at)}
            </span>
          {/if}
        </div>
      </div>
    </header>

    {#if profile.bio}
      <section class="bio">
        <h2 class="visually-hidden">Bio</h2>
        <p>{profile.bio}</p>
      </section>
    {/if}

    {#if fediAddress}
      <footer class="fedi">
        <span class="muted">Siga no fediverso:</span>
        <code>{fediAddress}</code>
        <button
          type="button"
          class="copy"
          onclick={copyFedi}
          aria-label={`Copiar endereço ${fediAddress}`}
        >
          {copied ? 'Copiado ✓' : 'Copiar'}
        </button>
      </footer>
    {/if}
  </article>
{/if}

<style>
  .profile {
    border: 1px solid var(--c-border);
    border-radius: var(--radius, 14px);
    overflow: hidden;
    background: var(--c-paper, #fff);
    box-shadow: var(--shadow);
  }
  .cover {
    height: clamp(120px, 28vw, 190px);
    background:
      linear-gradient(115deg, var(--c-navy, #0f172a) 0%, var(--c-green-dark, #115c2d) 60%, var(--c-green, #15803d) 100%);
    background-size: cover;
    background-position: center;
  }
  .head {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 1.25rem;
    align-items: flex-end;
    padding: 0 1.5rem 1.25rem;
    margin-top: -52px;
  }
  .avatar-lg {
    width: 116px;
    height: 116px;
    border-radius: 50%;
    object-fit: cover;
    background: var(--c-bg);
    border: 4px solid var(--c-paper, #fff);
    box-shadow: 0 2px 10px rgba(15, 23, 42, 0.12);
    flex-shrink: 0;
  }
  .avatar-fallback {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    font-size: 2.6rem;
    font-weight: 700;
    color: var(--c-green-dark);
    background: var(--c-green-soft);
  }
  .head-meta {
    padding-bottom: 0.4rem;
    min-width: 0;
  }
  .head h1 {
    margin: 0 0 0.1rem;
    font-size: clamp(1.3rem, 4vw, 1.6rem);
    overflow-wrap: anywhere;
  }
  .handle {
    margin: 0 0 0.55rem;
    color: var(--c-text-muted);
    font-size: 0.95rem;
    overflow-wrap: anywhere;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
  }
  .chip {
    border-radius: 999px;
    padding: 0.15rem 0.6rem;
    font-size: 0.8rem;
    font-weight: 600;
    white-space: nowrap;
  }
  .chip-ok {
    background: var(--c-green-soft, #e6f7ed);
    color: var(--c-green-dark, #115c2d);
    border: 1px solid #b7e4c7;
  }
  .chip-plain {
    background: var(--c-bg, #f2f4f7);
    color: var(--c-text-muted);
    border: 1px solid var(--c-border);
    font-weight: 500;
  }
  .chip-titulo {
    background: var(--c-blue-soft, #e6efff);
    color: var(--c-blue-dark, #143c78);
    border: 1px solid #b7d0ff;
  }
  .bio {
    padding: 0 1.5rem 1.5rem;
  }
  .bio p {
    margin: 0;
    white-space: pre-wrap;
    line-height: 1.55;
    overflow-wrap: anywhere;
  }
  .fedi {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.5rem;
    padding: 0.85rem 1.5rem;
    border-top: 1px solid var(--c-border);
    background: var(--c-bg);
    font-size: 0.88rem;
  }
  .fedi code {
    font-family: ui-monospace, SFMono-Regular, monospace;
    background: var(--c-paper);
    border: 1px solid var(--c-border);
    border-radius: 6px;
    padding: 0.1rem 0.45rem;
    overflow-wrap: anywhere;
  }
  .copy {
    font: inherit;
    font-size: 0.82rem;
    font-weight: 600;
    color: var(--c-green-dark);
    background: transparent;
    border: 1px solid var(--c-border);
    border-radius: 999px;
    padding: 0.2rem 0.7rem;
    cursor: pointer;
  }
  .copy:hover {
    background: var(--c-paper);
  }
  .state {
    text-align: center;
    padding: 2.5rem 1.5rem;
  }
  .state h2 {
    font-size: 1.25rem;
  }
  .state .btn {
    margin-top: 0.5rem;
  }

  /* Skeleton */
  .sk-cover {
    opacity: 0.35;
  }
  .sk-block,
  .sk-line {
    background: var(--c-bg);
    animation: pulse 1.4s ease-in-out infinite;
  }
  .sk-circle {
    display: inline-block;
    border-radius: 50%;
  }
  .sk-line {
    display: block;
    height: 0.9rem;
    border-radius: 6px;
    margin-bottom: 0.5rem;
  }
  .w50 {
    width: 50%;
  }
  .w30 {
    width: 30%;
  }
  @keyframes pulse {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.55;
    }
  }

  /* Mobile: empilha avatar sobre o texto, centralizado — nada de coluna estreita apertada. */
  @media (max-width: 560px) {
    .head {
      grid-template-columns: 1fr;
      justify-items: center;
      text-align: center;
      gap: 0.6rem;
      padding-inline: 1rem;
      margin-top: -46px;
    }
    .avatar-lg {
      width: 92px;
      height: 92px;
    }
    .chips {
      justify-content: center;
    }
    .bio {
      padding-inline: 1rem;
      text-align: center;
    }
    .fedi {
      padding-inline: 1rem;
      justify-content: center;
    }
  }
</style>
