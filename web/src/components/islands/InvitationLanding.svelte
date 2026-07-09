<script lang="ts">
  // Landing do convite de conta. Ler `?convite=TOKEN` da URL, chamar preview,
  // e apresentar CTA "Criar conta" que leva pra /cadastrar preservando o
  // token (rem. pra futuro consumo).
  import { onMount } from 'svelte';
  import { previewInvitation, type InvitationPreviewDto } from '../../lib/api';
  import Button from '../ui/Button.svelte';

  let loading = $state(true);
  let preview = $state<InvitationPreviewDto | null>(null);
  let token = $state('');

  onMount(async () => {
    const t = new URLSearchParams(window.location.search).get('convite')?.trim() ?? '';
    token = t;
    if (!t) {
      loading = false;
      return;
    }
    // Guarda pra o /confirmar-conta consumir depois do signup.
    try { localStorage.setItem('dsoc_invitation', t); } catch { /* storage blocked */ }
    const res = await previewInvitation(t);
    loading = false;
    if (res.ok && res.data) {
      preview = res.data;
    } else {
      preview = {
        valid: false,
        reason: 'error',
        invited_by_handle: null,
        invited_by_display_name: null,
        target_email: null,
      };
    }
  });

  function reasonMessage(r: string | null): string {
    switch (r) {
      case 'expired': return 'Este convite expirou.';
      case 'exhausted': return 'Este convite já foi usado no limite.';
      case 'not_found': return 'Convite não encontrado.';
      default: return 'Não foi possível validar o convite.';
    }
  }

  function signupHref() {
    return `/cadastrar?convite=${encodeURIComponent(token)}`;
  }
</script>

{#if loading}
  <p class="muted">Validando convite…</p>
{:else if !token}
  <h1>Convite não informado</h1>
  <p class="muted">
    O link precisa conter <code>?convite=TOKEN</code>. Se você recebeu um link
    encurtado por engano, peça o original a quem convidou.
  </p>
  <div class="row"><Button href="/">Ir pra home</Button></div>
{:else if preview && preview.valid}
  <h1>Você foi convidado(a)!</h1>
  {#if preview.invited_by_display_name || preview.invited_by_handle}
    <p class="muted">
      <strong>{preview.invited_by_display_name ?? preview.invited_by_handle}</strong>
      te convidou pra criar conta na DemocraciaBR.
    </p>
  {:else}
    <p class="muted">Alguém te convidou pra criar conta na DemocraciaBR.</p>
  {/if}
  <p>
    A instância é aberta — o convite não é obrigatório, mas serve pra
    quem convidou saber que você chegou. Prossiga com o cadastro normal
    abaixo.
  </p>
  {#if preview.target_email}
    <p class="hint muted small">
      Este convite é direcionado a <code>{preview.target_email}</code>. Use esse
      e-mail no cadastro.
    </p>
  {/if}
  <div class="row">
    <Button href={signupHref()} variant="primary">Criar conta</Button>
    <Button href="/entrar" variant="ghost">Já tenho conta</Button>
  </div>
{:else}
  <h1>Convite inválido</h1>
  <p class="muted">{reasonMessage(preview?.reason ?? null)}</p>
  <p>
    Você ainda pode criar conta livremente — a instância é aberta.
  </p>
  <div class="row">
    <Button href="/cadastrar" variant="primary">Criar conta</Button>
    <Button href="/" variant="ghost">Ir pra home</Button>
  </div>
{/if}

<style>
  h1 {
    margin: 0 0 0.5rem;
  }
  .row {
    display: flex;
    gap: 0.5rem;
    margin-top: 1.5rem;
    flex-wrap: wrap;
  }
  .small {
    font-size: 0.85rem;
  }
  code {
    background: var(--surface-2);
    padding: 2px 6px;
    border-radius: 4px;
    font-family: ui-monospace, SFMono-Regular, monospace;
  }
</style>
