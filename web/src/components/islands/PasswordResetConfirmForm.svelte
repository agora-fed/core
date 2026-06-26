<script lang="ts">
  // "Esqueci minha senha" — step 2: read the token from the URL, ask for the new password
  // twice, redeem against /auth/password-reset/confirm. On success the backend has also killed
  // every active session for the citizen — the user MUST log in fresh.
  import { onMount } from 'svelte';
  import { confirmPasswordReset } from '../../lib/api';

  let token = $state('');
  let password = $state('');
  let confirm = $state('');
  let busy = $state(false);
  let serverError = $state<string | null>(null);
  let done = $state(false);

  let passwordValid = $derived(password.length >= 8);
  let confirmValid = $derived(confirm === password && confirm.length > 0);
  let valid = $derived(token.length > 0 && passwordValid && confirmValid);

  onMount(() => {
    try {
      const params = new URLSearchParams(window.location.search);
      token = params.get('token') ?? '';
    } catch {
      /* no window in SSR; the form just shows the empty state */
    }
  });

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!valid || busy) return;
    busy = true;
    serverError = null;
    const res = await confirmPasswordReset(token, password);
    busy = false;
    if (res.success) {
      // Wipe any local greeting cache — the backend already invalidated cookies/sessions.
      try {
        localStorage.removeItem('dsoc_citizen');
        localStorage.removeItem('dsoc_handle');
      } catch {
        /* harmless */
      }
      done = true;
    } else {
      serverError =
        res.error?.message ??
        'O link pode ter expirado ou já ter sido usado. Peça um novo.';
    }
  }
</script>

{#if done}
  <div class="card success" role="status">
    <h2>Senha redefinida.</h2>
    <p class="muted">
      Suas sessões ativas foram encerradas por segurança. Entre na sua conta
      com a nova senha.
    </p>
    <a class="btn btn-primary btn-lg" href="/entrar">Entrar</a>
  </div>
{:else if !token}
  <div class="card error" role="alert">
    <h2>Link inválido.</h2>
    <p class="muted">
      Abra o link que veio por e-mail. Se já se perdeu,
      <a href="/recuperar-senha">peça um novo aqui</a>.
    </p>
  </div>
{:else}
  <form class="auth-form" onsubmit={submit} novalidate>
    <div class="field">
      <label for="np">Nova senha</label>
      <input
        id="np"
        class="input"
        type="password"
        autocomplete="new-password"
        bind:value={password}
        aria-invalid={password.length > 0 && !passwordValid}
        required
      />
      <p
        class={`hint ${password.length > 0 && !passwordValid ? 'hint-error' : 'muted'}`}
      >
        Mínimo de 8 caracteres.
      </p>
    </div>
    <div class="field">
      <label for="np2">Confirmar nova senha</label>
      <input
        id="np2"
        class="input"
        type="password"
        autocomplete="new-password"
        bind:value={confirm}
        aria-invalid={confirm.length > 0 && !confirmValid}
        required
      />
      {#if confirm.length > 0 && !confirmValid}
        <p class="hint hint-error">As senhas não conferem.</p>
      {/if}
    </div>
    <button
      class="btn btn-primary btn-lg block"
      type="submit"
      disabled={!valid || busy}
    >
      {busy ? 'Redefinindo…' : 'Redefinir senha'}
    </button>
    {#if serverError}
      <p class="note error" role="alert">{serverError}</p>
    {/if}
  </form>
{/if}

<style>
  .auth-form {
    display: block;
  }
  .block {
    width: 100%;
  }
  .success,
  .error {
    padding: 1.5rem;
    text-align: center;
  }
  .success h2,
  .error h2 {
    margin-top: 0;
  }
  .note {
    margin-top: 0.85rem;
    font-size: 0.92rem;
  }
  .note.error {
    color: var(--c-ignored);
  }
</style>
