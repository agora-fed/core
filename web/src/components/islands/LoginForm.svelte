<script lang="ts">
  // Login: e-mail + senha. On success the gateway sets a session cookie.
  import { login } from '../../lib/api';

  let email = $state('');
  let password = $state('');
  let busy = $state(false);
  let serverError = $state<string | null>(null);

  let emailValid = $derived(/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email));
  let valid = $derived(emailValid && password.length > 0);

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!valid || busy) return;
    serverError = null;
    busy = true;

    const res = await login(email, password);
    busy = false;

    if (res.success) {
      if (res.data?.citizen_id) {
        try {
          localStorage.setItem('dsoc_citizen', res.data.citizen_id);
          if (res.data.public_handle) {
            localStorage.setItem('dsoc_handle', res.data.public_handle);
          }
        } catch {
          /* storage may be blocked; session cookie still set */
        }
      }
      window.location.href = '/';
    } else {
      serverError = res.error?.message ?? 'E-mail ou senha incorretos.';
    }
  }
</script>

<form class="auth-form" onsubmit={submit} novalidate>
  <div class="field">
    <label for="l-email">E-mail</label>
    <input
      id="l-email"
      class="input"
      type="email"
      autocomplete="email"
      bind:value={email}
      aria-invalid={email.length > 0 && !emailValid}
      required
    />
  </div>

  <div class="field">
    <label for="l-password">Senha</label>
    <input
      id="l-password"
      class="input"
      type="password"
      autocomplete="current-password"
      bind:value={password}
      required
    />
  </div>

  <button
    class="btn btn-primary btn-lg block"
    type="submit"
    disabled={!valid || busy}
  >
    {busy ? 'Entrando…' : 'Entrar'}
  </button>

  {#if serverError}
    <p class="note error" role="alert">{serverError}</p>
  {/if}

  <p class="alt muted">
    Ainda não tem conta? <a href="/cadastrar">Criar conta</a>
  </p>
</form>

<style>
  .auth-form {
    display: block;
  }
  .block {
    width: 100%;
  }
  .note {
    margin: 0.85rem 0 0;
    font-size: 0.92rem;
  }
  .note.error {
    color: var(--c-ignored);
  }
  .alt {
    margin-top: 1rem;
    text-align: center;
    font-size: 0.95rem;
  }
</style>
