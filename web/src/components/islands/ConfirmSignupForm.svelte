<script lang="ts">
  // Passo 2 do cadastro (0.25.0-fediverso): lê o token da URL, redime em
  // /auth/register/confirm, e — em caso de sucesso — a resposta seta o
  // cookie de sessão e a gente redireciona pra /bem-vinda (ou o painel do
  // mandato, se dermos essa dica no futuro).
  //
  // Sem tela de "aguardando input" — assim que a página carrega, o clique
  // do e-mail já vale como confirmação. Se der erro (token vencido,
  // e-mail/CPF colidindo com outro cadastro concluído no meio, etc), a
  // gente mostra o motivo em português.
  import { onMount } from 'svelte';
  import { registerConfirm, associateInvitation } from '../../lib/api';
  import Alert from '../ui/Alert.svelte';
  import Button from '../ui/Button.svelte';

  let phase = $state<'reading' | 'submitting' | 'error' | 'done'>('reading');
  let serverError = $state<string | null>(null);
  let citizenId = $state<string | null>(null);

  onMount(async () => {
    let token = '';
    try {
      const params = new URLSearchParams(window.location.search);
      token = params.get('token') ?? '';
    } catch {
      /* SSR safety net */
    }
    if (!token) {
      serverError =
        'Link inválido. Peça um novo cadastro em /cadastrar.';
      phase = 'error';
      return;
    }
    phase = 'submitting';
    const res = await registerConfirm(token);
    if (res.success && res.data) {
      try {
        localStorage.setItem('dsoc_citizen', res.data.citizen_id);
        if (res.data.public_handle) {
          localStorage.setItem('dsoc_handle', res.data.public_handle);
        }
      } catch {
        /* storage may be blocked; cookie foi setado do lado do server */
      }
      citizenId = res.data.citizen_id;
      // 0.26.20: se o cidadão veio via /junte-se?convite=X, associa agora
      // que a conta existe e a sessão foi emitida.
      try {
        const invToken = localStorage.getItem('dsoc_invitation');
        if (invToken) {
          void associateInvitation(invToken).finally(() => {
            try { localStorage.removeItem('dsoc_invitation'); } catch { /* ignore */ }
          });
        }
      } catch { /* storage bloqueado */ }
      phase = 'done';
      // Delay curto pra o usuário ver a confirmação; aí redireciona.
      window.setTimeout(() => {
        window.location.href = '/bem-vinda';
      }, 1200);
    } else {
      serverError =
        res.error?.message ??
        'O link pode ter expirado ou já ter sido usado. Refaça o cadastro em /cadastrar.';
      phase = 'error';
    }
  });
</script>

{#if phase === 'reading' || phase === 'submitting'}
  <div class="state" aria-live="polite">
    <div class="spinner" aria-hidden="true"></div>
    <p>Confirmando seu cadastro…</p>
  </div>
{:else if phase === 'done'}
  <div class="state success" role="status">
    <h2>Conta ativada 🎉</h2>
    <p class="muted">
      Boas-vindas! Estamos te levando pra o painel inicial.
    </p>
    {#if citizenId}
      <p class="hint muted">id: {citizenId.slice(0, 8)}…</p>
    {/if}
  </div>
{:else}
  <div class="state error">
    <Alert tone="danger">{serverError ?? 'Não foi possível ativar a conta.'}</Alert>
    <div class="cta">
      <Button variant="primary" size="md" href="/cadastrar">
        Refazer cadastro
      </Button>
      <Button variant="ghost" size="md" href="/entrar">
        Já tenho conta
      </Button>
    </div>
  </div>
{/if}

<style>
  .state {
    display: grid;
    gap: var(--sp-3);
    text-align: center;
    padding: var(--sp-4);
  }
  .state h2 {
    margin: 0;
    font-size: var(--fs-lg);
    color: var(--text-1);
  }
  .state p {
    margin: 0;
    color: var(--text-2);
  }
  .state .hint {
    font-size: var(--fs-xs);
  }
  .spinner {
    width: 32px;
    height: 32px;
    border-radius: 50%;
    border: 3px solid var(--border-subtle);
    border-top-color: var(--accent);
    animation: spin 0.9s linear infinite;
    margin: 0 auto;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  .cta {
    display: flex;
    gap: var(--sp-2);
    justify-content: center;
    flex-wrap: wrap;
  }
</style>
