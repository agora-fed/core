<script lang="ts">
  // Telefone + verificação por OTP SMS (ÁGORA F5, #62). Opt-in. Consome /me/phone (EN).
  import { onMount } from 'svelte';
  import { getPhoneStatus, setPhone, verifyPhone } from '../../lib/api';
  import { toast } from '../../lib/toasts';

  let loading = $state(true);
  let phone = $state('');
  let verified = $state(false);
  let stage = $state<'idle' | 'code'>('idle');
  let code = $state('');
  let busy = $state(false);

  async function reload() {
    loading = true;
    const res = await getPhoneStatus();
    loading = false;
    if (res.success && res.data) {
      phone = res.data.phone ?? '';
      verified = res.data.verified;
    }
  }
  onMount(reload);

  async function requestCode(e: Event) {
    e.preventDefault();
    if (!phone.trim()) return toast.error('Informe o telefone');
    busy = true;
    const res = await setPhone(phone.trim());
    busy = false;
    if (!res.success) return toast.error(res.error?.message ?? 'Não foi possível enviar o código');
    stage = 'code';
    toast.success('Código enviado por SMS');
  }

  async function confirm(e: Event) {
    e.preventDefault();
    busy = true;
    const res = await verifyPhone(code.trim());
    busy = false;
    if (!res.success) return toast.error(res.error?.message ?? 'Código incorreto');
    verified = true;
    stage = 'idle';
    code = '';
    toast.success('Telefone verificado');
  }
</script>

<div class="ph">
  <p class="intro">
    Verificar seu telefone é <strong>opcional</strong>. Serve como alternativa de recuperação
    (2FA por SMS) caso perca o acesso ao e-mail — <strong>não é o método recomendado</strong>
    (prefira um app autenticador, em breve), mas é uma opção. Também permite receber alertas por
    SMS onde você autorizar.
  </p>

  {#if loading}
    <p class="muted">Carregando…</p>
  {:else if verified}
    <p class="ok">✓ Telefone verificado: <strong>{phone}</strong></p>
    <form class="row" onsubmit={requestCode}>
      <input bind:value={phone} class="input" placeholder="+55 11 98765-4321" />
      <button class="btn" disabled={busy}>Trocar / reverificar</button>
    </form>
  {:else if stage === 'idle'}
    <form class="row" onsubmit={requestCode}>
      <input bind:value={phone} class="input" placeholder="+55 11 98765-4321" required />
      <button class="btn" disabled={busy}>Enviar código por SMS</button>
    </form>
  {:else}
    <p class="muted small">Enviamos um código de 6 dígitos para {phone}.</p>
    <form class="row" onsubmit={confirm}>
      <input bind:value={code} class="input" placeholder="000000" inputmode="numeric" maxlength="6" required />
      <button class="btn" disabled={busy}>Confirmar</button>
      <button type="button" class="link" onclick={() => (stage = 'idle')}>voltar</button>
    </form>
  {/if}
</div>

<style>
  .ph { max-width: 34rem; }
  .intro { color: var(--text-2); line-height: var(--lh-relaxed); background: var(--surface-2); border-left: 3px solid var(--accent); border-radius: var(--r-sm); padding: var(--sp-3) var(--sp-4); margin: 0 0 var(--sp-4); }
  .ok { color: var(--accent-strong); font-weight: var(--fw-semibold); margin: 0 0 var(--sp-3); }
  .row { display: flex; flex-wrap: wrap; gap: var(--sp-2); align-items: center; }
  .input { padding: var(--sp-2) var(--sp-3); border: 1px solid var(--border-subtle); border-radius: var(--r-sm); background: var(--surface-1); color: var(--text-1); font-size: var(--fs-sm); }
  .btn { padding: var(--sp-2) var(--sp-4); border-radius: var(--r-sm); border: 1px solid var(--accent); background: var(--accent); color: #fff; font-weight: var(--fw-semibold); font-size: var(--fs-sm); cursor: pointer; }
  .btn:disabled { opacity: 0.6; }
  .link { background: none; border: none; color: var(--text-3); cursor: pointer; font-size: var(--fs-sm); text-decoration: underline; }
  .muted { color: var(--text-3); }
  .small { font-size: var(--fs-sm); }
</style>
