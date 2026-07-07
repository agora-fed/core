<script lang="ts">
  // Login: e-mail + senha. On success the gateway sets a session cookie.
  import { login } from '../../lib/api';
  import Input from '../ui/Input.svelte';
  import Button from '../ui/Button.svelte';
  import Alert from '../ui/Alert.svelte';
  import Icon from '../ui/Icon.svelte';

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
  <Input
    id="l-email"
    label="E-mail"
    type="email"
    autocomplete="email"
    bind:value={email}
    required
    leading={emailIcon}
    error={email.length > 0 && !emailValid
      ? 'Endereço de e-mail inválido.'
      : undefined}
  />

  <Input
    id="l-password"
    label="Senha"
    type="password"
    autocomplete="current-password"
    bind:value={password}
    required
    leading={lockIcon}
  />

  {#snippet emailIcon()}
    <Icon name="at" size={16} />
  {/snippet}
  {#snippet lockIcon()}
    <Icon name="lock" size={16} />
  {/snippet}

  <Button
    type="submit"
    variant="primary"
    size="lg"
    fullWidth
    loading={busy}
    disabled={!valid}
  >
    Entrar
  </Button>

  {#if serverError}
    <div class="err">
      <Alert tone="danger">{serverError}</Alert>
    </div>
  {/if}

  <p class="alt muted">
    Ainda não tem conta? <a href="/cadastrar">Criar conta</a>
  </p>
  <p class="alt muted">
    <a href="/recuperar-senha">Esqueci minha senha</a>
  </p>
</form>

<style>
  .auth-form {
    display: block;
  }
  .err {
    margin-top: var(--sp-3);
  }
  .alt {
    margin-top: var(--sp-3);
    text-align: center;
    font-size: var(--fs-sm);
    color: var(--text-3);
  }
  .alt a {
    color: var(--accent);
    font-weight: var(--fw-medium);
  }
  .alt a:hover {
    color: var(--accent-strong);
  }
</style>
