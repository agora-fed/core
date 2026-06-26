<script lang="ts">
  // Profile editor — display name, bio, handle, privacy toggle. Reads /me on mount and PATCHes
  // /me on submit; on success refreshes the cached handle in localStorage so the header greeting
  // updates without a reload.
  import { onMount } from 'svelte';
  import {
    getMyProfile,
    updateMyProfile,
    type ProfileDto,
  } from '../../lib/api';

  let loading = $state(true);
  let profile = $state<ProfileDto | null>(null);
  let loadError = $state<string | null>(null);

  let displayName = $state('');
  let bio = $state('');
  let handle = $state('');
  let isPublic = $state(false);

  let busy = $state(false);
  let status = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);

  // Handle is the only field with a server-side regex; mirror the rule client-side so the user
  // sees feedback before the round trip. Empty handle = leave unset / clear.
  let handleValid = $derived(
    handle === '' ||
      (/^[A-Za-z0-9_.-]{3,32}$/.test(handle) && !handle.includes('..')),
  );
  let bioWithin = $derived(bio.length <= 500);
  let displayNameWithin = $derived(displayName.length <= 80);
  let canSave = $derived(
    !loading && !busy && handleValid && bioWithin && displayNameWithin,
  );

  onMount(async () => {
    const res = await getMyProfile();
    loading = false;
    if (res.success && res.data) {
      profile = res.data;
      displayName = res.data.display_name ?? '';
      bio = res.data.bio ?? '';
      handle = res.data.handle ?? '';
      isPublic = res.data.is_public;
    } else if (res.error?.code === 'http_401') {
      loadError = 'Você precisa entrar na sua conta para abrir as configurações.';
    } else {
      loadError = res.error?.message ?? 'Não foi possível carregar seu perfil.';
    }
  });

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!canSave) return;
    busy = true;
    status = null;
    const res = await updateMyProfile({
      display_name: displayName,
      bio,
      handle,
      is_public: isPublic,
    });
    busy = false;
    if (res.success && res.data) {
      profile = res.data;
      // Update the header greeting cache so AuthMenu picks it up on next mount.
      try {
        const greeting = res.data.handle ?? res.data.public_handle;
        localStorage.setItem('dsoc_handle', greeting);
      } catch {
        /* storage may be blocked; harmless */
      }
      status = { kind: 'ok', text: 'Perfil atualizado.' };
    } else {
      status = {
        kind: 'error',
        text: res.error?.message ?? 'Não foi possível salvar.',
      };
    }
  }
</script>

{#if loading}
  <p class="muted">Carregando…</p>
{:else if loadError}
  <div class="card" role="alert">
    <p>{loadError}</p>
    <p class="muted">
      <a href="/entrar">Entrar na minha conta</a>
    </p>
  </div>
{:else if profile}
  <form class="profile-form" onsubmit={submit} novalidate>
    <div class="field">
      <label for="p-display">Nome para exibição</label>
      <input
        id="p-display"
        class="input"
        type="text"
        bind:value={displayName}
        maxlength="80"
        placeholder="Como você quer ser chamada"
        aria-invalid={!displayNameWithin}
      />
      <p class="hint muted">Aparece no cabeçalho e nas suas propostas.</p>
    </div>

    <div class="field">
      <label for="p-handle">Nome de usuário (@)</label>
      <div class="handle-wrap">
        <span class="prefix" aria-hidden="true">@</span>
        <input
          id="p-handle"
          class="input"
          type="text"
          bind:value={handle}
          maxlength="32"
          placeholder="ana.lima"
          autocomplete="off"
          aria-invalid={handle.length > 0 && !handleValid}
        />
      </div>
      {#if handle.length > 0 && !handleValid}
        <p class="hint hint-error">
          Use 3–32 caracteres: letras, números, ponto, hífen ou _ (sem dois pontos seguidos).
        </p>
      {:else}
        <p class="hint muted">
          Identifica você no fediverso quando seu perfil for público.
        </p>
      {/if}
    </div>

    <div class="field">
      <label for="p-bio">Bio</label>
      <textarea
        id="p-bio"
        class="input"
        rows="4"
        bind:value={bio}
        placeholder="Conte em poucas palavras quem você é e por que está aqui."
        aria-invalid={!bioWithin}
      ></textarea>
      <p class={`hint ${bioWithin ? 'muted' : 'hint-error'}`}>
        {bio.length}/500
      </p>
    </div>

    <div class="field privacy">
      <label class="toggle">
        <input type="checkbox" bind:checked={isPublic} />
        <span>
          <strong>Tornar meu perfil público no fediverso</strong>
          <span class="hint muted block">
            Padrão: privado. Marcando esta opção, outras pessoas (inclusive em
            outras instâncias) podem te encontrar pelo @ e te seguir.
          </span>
        </span>
      </label>
    </div>

    <button
      class="btn btn-primary btn-lg"
      type="submit"
      disabled={!canSave}
    >
      {busy ? 'Salvando…' : 'Salvar alterações'}
    </button>

    {#if status}
      <p class={`note ${status.kind}`} role="status">{status.text}</p>
    {/if}
  </form>
{/if}

<style>
  .profile-form {
    display: block;
  }
  .handle-wrap {
    display: flex;
    align-items: stretch;
    border: 1px solid var(--c-border);
    border-radius: 8px;
    overflow: hidden;
    background: var(--c-paper);
  }
  .handle-wrap .prefix {
    display: flex;
    align-items: center;
    padding: 0 0.7rem;
    color: var(--c-text-muted);
    background: var(--c-bg);
    border-right: 1px solid var(--c-border);
    font-weight: 600;
  }
  .handle-wrap .input {
    border: 0;
    border-radius: 0;
    background: transparent;
    flex: 1;
  }
  .privacy {
    border-top: 1px solid var(--c-border);
    padding-top: 1.25rem;
    margin-top: 0.5rem;
  }
  .toggle {
    display: flex;
    align-items: flex-start;
    gap: 0.75rem;
    cursor: pointer;
  }
  .toggle input {
    margin-top: 0.3rem;
    width: 1.05rem;
    height: 1.05rem;
    accent-color: var(--c-green-dark);
  }
  .block {
    display: block;
    margin-top: 0.25rem;
  }
  textarea.input {
    resize: vertical;
  }
  .note {
    margin-top: 0.85rem;
    font-size: 0.92rem;
  }
  .note.ok {
    color: var(--c-green-dark);
  }
  .note.error {
    color: var(--c-ignored);
  }
</style>
