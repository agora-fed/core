<script lang="ts">
  // Login: e-mail + senha. On success the gateway sets a session cookie.
  import { onMount } from 'svelte';
  import { apiGetCredentialed, login, getGovbrStatus, getSsoStatus } from '../../lib/api';
  import SsoButton from './SsoButton.svelte';
  import Input from '../ui/Input.svelte';
  import Button from '../ui/Button.svelte';
  import Alert from '../ui/Alert.svelte';
  import Icon from '../ui/Icon.svelte';

  let email = $state('');
  let password = $state('');
  let busy = $state(false);
  let serverError = $state<string | null>(null);
  let govbrEnabled = $state(false);
  let passwordHidden = $state(false);

  const SSO_ERRORS: Record<string, string> = {
    organizacao: 'Esta conta não pertence a esta comunidade.',
    conta: 'Sua conta não pode entrar agora. Fale com a moderação.',
  };

  /** The session cookie is HttpOnly, so ask the API who signed in and go on. */
  async function finishSso(params: URLSearchParams) {
    const who = await apiGetCredentialed<{ citizen_id: string; public_handle?: string }>(
      '/api/v1/me/whoami',
    );
    if (!who.success || !who.data?.citizen_id) {
      serverError = 'Não foi possível concluir o login. Tente novamente.';
      return;
    }
    try {
      localStorage.setItem('dsoc_citizen', who.data.citizen_id);
    } catch {
      /* storage may be blocked; the session cookie is still set */
    }
    const next = params.get('next') ?? '/';
    window.location.replace(next.startsWith('/') && !next.startsWith('//') ? next : '/');
  }

  onMount(async () => {
    const query = new URLSearchParams(window.location.search);
    if (query.get('sso') === 'ok') return finishSso(query);
    const ssoError = query.get('sso_erro');
    if (ssoError) {
      serverError = SSO_ERRORS[ssoError] ?? 'Não foi possível entrar com essa conta. Tente novamente.';
    }
    const sso = await getSsoStatus();
    // `?admin=1` keeps the password form reachable if the provider is down.
    passwordHidden = sso.ok && sso.data?.only === true && query.get('admin') !== '1';
    const r = await getGovbrStatus();
    govbrEnabled = r.ok && r.data?.enabled === true;
    // Erro devolvido pelo callback do gov.br (query ?govbr_erro=...).
    try {
      const p = new URLSearchParams(window.location.search);
      const g = p.get('govbr_erro');
      if (g) {
        serverError = `Falha no login gov.br (${g}). Tente com e-mail e senha ou entre em contato.`;
      }
    } catch {}
  });

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
      // Respect ?next=<path> so the OAuth /oauth/authorize consent flow can
      // round-trip through /entrar. Only allow SAME-ORIGIN paths (no
      // protocol/host swap) — protection against open-redirect abuse.
      let dest = '/';
      try {
        const params = new URLSearchParams(window.location.search);
        const next = params.get('next');
        if (next && next.startsWith('/') && !next.startsWith('//')) {
          dest = next;
        }
      } catch {}
      window.location.href = dest;
    } else {
      serverError = res.error?.message ?? 'E-mail ou senha incorretos.';
    }
  }
</script>

<SsoButton />

{#if govbrEnabled}
  <a class="govbr-btn" href="/auth/govbr/start" aria-label="Entrar com gov.br">
    <span class="govbr-mark" aria-hidden="true">gov.br</span>
    <span>Entrar com gov.br</span>
  </a>
  <p class="or muted">ou</p>
{/if}

{#if !passwordHidden}
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
{/if}

<style>
  .govbr-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: var(--sp-2);
    width: 100%;
    padding: 14px 16px;
    background: #1351b4; /* Azul gov.br oficial */
    color: white;
    border-radius: var(--r-base);
    text-decoration: none;
    font-weight: var(--fw-semibold);
    font-size: var(--fs-md);
    margin-bottom: var(--sp-3);
    transition: background var(--dur-fast) var(--ease-out);
  }
  .govbr-btn:hover {
    background: #0e3f8a;
  }
  .govbr-mark {
    background: white;
    color: #1351b4;
    padding: 2px 8px;
    border-radius: var(--r-sm);
    font-size: var(--fs-sm);
    font-weight: var(--fw-bold);
    letter-spacing: 0.02em;
  }
  .or {
    text-align: center;
    font-size: var(--fs-sm);
    margin: var(--sp-2) 0 var(--sp-3);
    position: relative;
  }
  .or::before,
  .or::after {
    content: '';
    display: inline-block;
    height: 1px;
    background: var(--border-subtle);
    width: 35%;
    vertical-align: middle;
    margin: 0 var(--sp-2);
  }
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
