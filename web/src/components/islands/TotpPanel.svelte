<script lang="ts">
  // 2FA por TOTP (app autenticador) — ÁGORA F6 (#63). Recomendado. Consome /me/2fa/totp (EN).
  import { onMount } from 'svelte';
  import { getTotpStatus, totpSetup, totpEnable, totpDisable } from '../../lib/api';
  import { toast } from '../../lib/toasts';

  let loading = $state(true);
  let enabled = $state(false);
  let stage = $state<'idle' | 'setup' | 'disable'>('idle');
  let secret = $state('');
  let uri = $state('');
  let code = $state('');
  let recovery = $state<string[]>([]);
  let busy = $state(false);

  async function reload() {
    loading = true;
    const res = await getTotpStatus();
    loading = false;
    if (res.success && res.data) enabled = res.data.enabled;
  }
  onMount(reload);

  async function startSetup() {
    busy = true;
    const res = await totpSetup();
    busy = false;
    if (!res.success) return toast.error(res.error?.message ?? 'Não foi possível iniciar');
    secret = res.data?.secret ?? '';
    uri = res.data?.uri ?? '';
    stage = 'setup';
  }

  async function confirmEnable(e: Event) {
    e.preventDefault();
    busy = true;
    const res = await totpEnable(code.trim());
    busy = false;
    if (!res.success) return toast.error(res.error?.message ?? 'Código incorreto');
    recovery = res.data?.recovery_codes ?? [];
    enabled = true;
    stage = 'idle';
    code = '';
    toast.success('2FA (TOTP) ativado');
  }

  async function confirmDisable(e: Event) {
    e.preventDefault();
    busy = true;
    const res = await totpDisable(code.trim());
    busy = false;
    if (!res.success) return toast.error(res.error?.message ?? 'Código incorreto');
    enabled = false;
    stage = 'idle';
    code = '';
    recovery = [];
    toast.success('2FA desativado');
  }
</script>

<div class="totp">
  <p class="intro">
    <strong>Recomendado.</strong> Use um app autenticador (Google Authenticator, Aegis, etc.) para
    gerar um código de 6 dígitos que muda a cada 30s — o 2FA mais seguro.
  </p>

  {#if loading}
    <p class="muted">Carregando…</p>
  {:else if enabled && recovery.length === 0}
    <p class="ok">✓ 2FA por TOTP está ativo.</p>
    {#if stage !== 'disable'}
      <button class="btn ghost" onclick={() => (stage = 'disable')}>Desativar</button>
    {:else}
      <form class="row" onsubmit={confirmDisable}>
        <input bind:value={code} class="input" placeholder="Código do app" inputmode="numeric" maxlength="6" required />
        <button class="btn" disabled={busy}>Confirmar desativação</button>
      </form>
    {/if}
  {:else if recovery.length > 0}
    <p class="ok">✓ 2FA ativado! Guarde estes <strong>códigos de recuperação</strong> (mostrados só agora):</p>
    <ul class="codes">
      {#each recovery as c (c)}<li>{c}</li>{/each}
    </ul>
    <p class="muted small">Cada código serve uma vez, caso você perca o app. Guarde em lugar seguro.</p>
  {:else if stage === 'idle'}
    <button class="btn" onclick={startSetup} disabled={busy}>Ativar 2FA (TOTP)</button>
  {:else}
    <p class="muted small">
      Escaneie o QR no seu app, ou adicione a chave manualmente:
    </p>
    <p class="secret"><code>{secret}</code></p>
    <p class="muted small"><a href={uri}>Abrir no app autenticador</a></p>
    <form class="row" onsubmit={confirmEnable}>
      <input bind:value={code} class="input" placeholder="Código do app" inputmode="numeric" maxlength="6" required />
      <button class="btn" disabled={busy}>Confirmar e ativar</button>
    </form>
  {/if}
</div>

<style>
  .totp { max-width: 34rem; margin-bottom: var(--sp-6); }
  .intro { color: var(--text-2); line-height: var(--lh-relaxed); background: var(--surface-2); border-left: 3px solid var(--accent); border-radius: var(--r-sm); padding: var(--sp-3) var(--sp-4); margin: 0 0 var(--sp-4); }
  .ok { color: var(--accent-strong); font-weight: var(--fw-semibold); margin: 0 0 var(--sp-3); }
  .row { display: flex; flex-wrap: wrap; gap: var(--sp-2); align-items: center; margin-top: var(--sp-2); }
  .input { padding: var(--sp-2) var(--sp-3); border: 1px solid var(--border-subtle); border-radius: var(--r-sm); background: var(--surface-1); color: var(--text-1); font-size: var(--fs-sm); }
  .btn { padding: var(--sp-2) var(--sp-4); border-radius: var(--r-sm); border: 1px solid var(--accent); background: var(--accent); color: #fff; font-weight: var(--fw-semibold); font-size: var(--fs-sm); cursor: pointer; }
  .btn.ghost { background: var(--surface-1); color: var(--text-1); border-color: var(--border-subtle); }
  .btn:disabled { opacity: 0.6; }
  .secret code { font-size: var(--fs-lg); letter-spacing: 2px; background: var(--surface-2); padding: var(--sp-2) var(--sp-3); border-radius: var(--r-sm); display: inline-block; }
  .codes { list-style: none; padding: 0; margin: var(--sp-2) 0; display: grid; grid-template-columns: repeat(2, max-content); gap: 4px var(--sp-5); }
  .codes li { font-family: monospace; font-size: var(--fs-md); }
  .muted { color: var(--text-3); }
  .small { font-size: var(--fs-sm); }
</style>
