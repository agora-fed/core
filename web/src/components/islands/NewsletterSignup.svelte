<script lang="ts">
  // Captação de audiência (0.35.0): o form "receba novidades" do rodapé.
  // Consent LGPD explícito no microcopy; honeypot invisível contra bots.
  import { subscribeAudience } from '../../lib/api';

  let email = $state('');
  let name = $state('');
  let website = $state(''); // honeypot — humano não vê
  let busy = $state(false);
  let done = $state(false);
  let error = $state<string | null>(null);

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    if (busy || !email.trim()) return;
    busy = true;
    error = null;
    const res = await subscribeAudience({
      email: email.trim(),
      name: name.trim() || undefined,
      website,
    });
    busy = false;
    if (res.success) {
      done = true;
    } else {
      error = res.error?.message ?? 'Não foi possível se inscrever agora.';
    }
  }
</script>

<div class="signup">
  {#if done}
    <p class="ok">✅ Pronto! Você vai receber as novidades da DemocraciaBR.</p>
  {:else}
    <form onsubmit={submit}>
      <div class="fields">
        <input
          class="input"
          type="text"
          placeholder="Seu nome (opcional)"
          autocomplete="name"
          bind:value={name}
        />
        <input
          class="input"
          type="email"
          required
          placeholder="seu@email.com"
          autocomplete="email"
          bind:value={email}
        />
        <!-- Honeypot: fora da visão e do tab-order. -->
        <input
          class="hp"
          type="text"
          tabindex="-1"
          autocomplete="off"
          aria-hidden="true"
          bind:value={website}
        />
        <button type="submit" class="btn" disabled={busy}>
          {busy ? 'Enviando…' : 'Quero receber'}
        </button>
      </div>
      {#if error}<p class="err">{error}</p>{/if}
      <p class="consent">
        Ao se inscrever você concorda em receber novidades da DemocraciaBR por
        e-mail. Todo e-mail tem link de descadastro de 1 clique.
        <a href="/privacidade">Política de privacidade</a>.
      </p>
    </form>
  {/if}
</div>

<style>
  .signup {
    max-width: 560px;
  }
  .fields {
    display: flex;
    gap: var(--sp-2);
    flex-wrap: wrap;
  }
  .input {
    flex: 1;
    min-width: 160px;
    padding: 10px 12px;
    border: 1px solid var(--border, #cbd5e1);
    border-radius: var(--r-sm, 8px);
    background: var(--surface-1, #fff);
    color: inherit;
    font-size: var(--fs-sm, 0.95rem);
  }
  .btn {
    padding: 10px 18px;
    border: none;
    border-radius: var(--r-sm, 8px);
    background: var(--accent-strong, #15803d);
    color: #fff;
    font-weight: 600;
    cursor: pointer;
  }
  .btn:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .hp {
    position: absolute;
    left: -9999px;
    width: 1px;
    height: 1px;
    opacity: 0;
  }
  .consent {
    margin-top: var(--sp-2);
    font-size: var(--fs-xs, 0.78rem);
    color: var(--text-2, #64748b);
  }
  .consent a {
    color: var(--accent-strong, #15803d);
  }
  .ok {
    font-weight: 600;
    color: var(--accent-strong, #15803d);
  }
  .err {
    margin-top: var(--sp-1);
    color: var(--danger, #b91c1c);
    font-size: var(--fs-sm, 0.9rem);
  }
</style>
