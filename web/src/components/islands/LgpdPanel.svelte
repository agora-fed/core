<script lang="ts">
  // Painel LGPD em /configuracoes → aba "LGPD".
  // Duas ações: baixar JSON dos meus dados; excluir conta.
  import { exportMyData, deleteMyAccount } from '../../lib/api';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';
  import Alert from '../ui/Alert.svelte';

  let exporting = $state(false);
  let deleting = $state(false);
  let msg = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);

  async function doExport() {
    if (exporting) return;
    exporting = true;
    msg = null;
    const res = await exportMyData();
    exporting = false;
    if (!res.success || !res.data) {
      msg = {
        kind: 'error',
        text: res.error?.message ?? 'Não foi possível exportar.',
      };
      return;
    }
    // Download JSON.
    const blob = new Blob([JSON.stringify(res.data, null, 2)], {
      type: 'application/json',
    });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    const now = new Date();
    a.download = `democracia-br-meus-dados-${now.toISOString().slice(0, 10)}.json`;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
    msg = { kind: 'ok', text: 'Exportado. Confira sua pasta de downloads.' };
  }

  async function doDelete() {
    if (deleting) return;
    const confirmation = prompt(
      'Isto é IRREVERSÍVEL. Sua conta será excluída, dados pessoais apagados, ' +
        'e conteúdo publicado (propostas, votos) fica anonimizado.\n\n' +
        'Digite EXCLUIR MINHA CONTA para confirmar:',
    );
    if (confirmation !== 'EXCLUIR MINHA CONTA') {
      msg = { kind: 'error', text: 'Cancelado — confirmação não bateu.' };
      return;
    }
    deleting = true;
    msg = null;
    const res = await deleteMyAccount();
    if (!res.success) {
      deleting = false;
      msg = {
        kind: 'error',
        text: res.error?.message ?? 'Não foi possível excluir.',
      };
      return;
    }
    // Limpa localStorage e redireciona.
    try {
      for (const k of [
        'dsoc_citizen', 'dsoc_handle', 'dsoc_name', 'dsoc_avatar',
        'dsoc_is_admin',
      ]) {
        localStorage.removeItem(k);
      }
    } catch {}
    window.location.href = '/?conta-excluida=1';
  }
</script>

<Card>
  <h3>Exportar meus dados</h3>
  <p class="muted">
    Baixe um arquivo JSON com <strong>todos os seus dados pessoais</strong>
    guardados na plataforma: perfil, e-mails, sessões, propostas, votos,
    emendas, notificações, subscrições push. Formato portável (LGPD art. 18 V).
  </p>
  <div class="row">
    <Button variant="primary" onclick={doExport} loading={exporting}>
      Baixar JSON dos meus dados
    </Button>
  </div>
</Card>

<div class="gap"></div>

<Card>
  <h3>Excluir minha conta</h3>
  <p class="muted">
    Direito de eliminação (LGPD art. 18 VI). Ao confirmar:
  </p>
  <ul class="muted">
    <li>Seu perfil, e-mail, senha, CPF, título de eleitor e vínculo gov.br são apagados.</li>
    <li>Todas as sessões ativas são encerradas.</li>
    <li>Conteúdo já publicado (propostas, votos, comentários) permanece no registro histórico — mas anonimizado. Interesse público em accountability parlamentar prevalece (LGPD art. 16).</li>
    <li>Não é reversível.</li>
  </ul>
  <div class="row">
    <Button variant="danger" onclick={doDelete} loading={deleting}>
      Excluir minha conta permanentemente
    </Button>
  </div>
</Card>

{#if msg}
  <div class="gap"></div>
  <Alert tone={msg.kind === 'ok' ? 'success' : 'danger'}>
    {msg.text}
  </Alert>
{/if}

<style>
  h3 {
    margin: 0 0 var(--sp-2);
    color: var(--text-1);
  }
  .muted {
    color: var(--text-2);
    line-height: var(--lh-relaxed);
  }
  ul {
    padding-left: 1.4rem;
    display: grid;
    gap: 4px;
    margin: 0 0 var(--sp-3);
  }
  .row {
    display: flex;
    gap: var(--sp-2);
    margin-top: var(--sp-3);
  }
  .gap {
    height: var(--sp-4);
  }
</style>
