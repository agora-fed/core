<script lang="ts">
  // Tabbed settings shell. Reads/writes the active tab to
  // window.location.hash so a "Reload" preserves the section the user was on
  // (also gives every section a shareable URL: /configuracoes#seguranca).
  import { onMount } from 'svelte';
  import Tabs from '../ui/Tabs.svelte';
  import Card from '../ui/Card.svelte';
  import Icon from '../ui/Icon.svelte';
  import ProfileForm from './ProfileForm.svelte';
  import ThemeToggle from './ThemeToggle.svelte';
  import SessionsList from './SessionsList.svelte';
  import ChangePasswordForm from './ChangePasswordForm.svelte';
  import AuthorizedApps from './AuthorizedApps.svelte';
  import FediverseSearch from './FediverseSearch.svelte';
  import TituloEleitorForm from './TituloEleitorForm.svelte';
  import LgpdPanel from './LgpdPanel.svelte';
  import CampaignConsentPanel from './CampaignConsentPanel.svelte';
  import FiltersPanel from './FiltersPanel.svelte';
  import PreferencesPanel from './PreferencesPanel.svelte';
  import ImportPanel from './ImportPanel.svelte';

  type TabId =
    | 'perfil'
    | 'identidade'
    | 'aparencia'
    | 'seguranca'
    | 'aplicativos'
    | 'fediverso'
    | 'preferencias'
    | 'filtros'
    | 'importar'
    | 'campanha'
    | 'lgpd';

  const tabs: { id: TabId; label: string }[] = [
    { id: 'perfil', label: 'Perfil' },
    { id: 'identidade', label: 'Identidade' },
    { id: 'aparencia', label: 'Aparência' },
    { id: 'seguranca', label: 'Segurança' },
    { id: 'preferencias', label: 'Preferências' },
    { id: 'aplicativos', label: 'Aplicativos' },
    { id: 'fediverso', label: 'Fediverso' },
    { id: 'filtros', label: 'Filtros' },
    { id: 'importar', label: 'Importar' },
    { id: 'campanha', label: 'Campanha' },
    { id: 'lgpd', label: 'LGPD' },
  ];

  let active = $state<TabId>('perfil');

  onMount(() => {
    const h = window.location.hash.replace('#', '') as TabId;
    if (tabs.some((t) => t.id === h)) active = h;
  });

  function select(id: string) {
    active = id as TabId;
    if (typeof history !== 'undefined') {
      history.replaceState(null, '', `#${id}`);
    }
  }
</script>

<div class="shell">
  <Tabs {tabs} bind:active onselect={select} />

  <div class="pane" role="tabpanel">
    {#if active === 'perfil'}
      <section class="section">
        <header class="s-head">
          <h2>Seu perfil</h2>
          <p class="muted">
            Nome, bio, foto e capa. O perfil nasce <strong>privado</strong> —
            marque a última opção para federar no fediverso.
          </p>
        </header>
        <ProfileForm />
      </section>
    {:else if active === 'identidade'}
      <section class="section">
        <header class="s-head">
          <h2>Identidade cívica</h2>
          <p class="muted">
            Vincule seu <strong>título de eleitor</strong> pra participar de decisões
            vinculantes. A validação é feita algoritmicamente pelo próprio TSE
            (dígitos verificadores). Guardamos apenas os 4 últimos dígitos por padrão
            de segurança e LGPD.
          </p>
        </header>
        <TituloEleitorForm />
      </section>
    {:else if active === 'aparencia'}
      <section class="section">
        <header class="s-head">
          <h2>Aparência</h2>
          <p class="muted">
            Tema escuro, claro ou seguindo o sistema operacional. A preferência
            fica guardada neste dispositivo.
          </p>
        </header>
        <Card>
          <div class="theme-row">
            <div>
              <strong>Tema visual</strong>
              <p class="muted small">
                Aplica em todo o site imediatamente. Auto = respeita a preferência do sistema.
              </p>
            </div>
            <ThemeToggle />
          </div>
        </Card>
      </section>
    {:else if active === 'seguranca'}
      <section class="section">
        <header class="s-head">
          <h2>Segurança</h2>
          <p class="muted">
            Sua senha protege sua conta e sua chave de identidade no fediverso.
            Trocando a senha, os outros navegadores e apps precisam entrar de novo.
          </p>
        </header>
        <Card>
          <h3 class="sub"><Icon name="lock" size={18} /> Trocar senha</h3>
          <ChangePasswordForm />
        </Card>
        <div class="spacer"></div>
        <SessionsList />
      </section>
    {:else if active === 'aplicativos'}
      <section class="section">
        <header class="s-head">
          <h2>Aplicativos conectados</h2>
          <p class="muted">
            Clientes Mastodon e integrações que acessam sua conta via OAuth. Desconecte
            um app para revogar imediatamente o acesso dele.
          </p>
        </header>
        <AuthorizedApps />
      </section>
    {:else if active === 'fediverso'}
      <section class="section">
        <header class="s-head">
          <h2>Descobrir no fediverso</h2>
          <p class="muted">
            Busque perfis em outras instâncias (Mastodon, Pleroma, Misskey…) e comece a segui-los.
          </p>
        </header>
        <FediverseSearch />
      </section>
    {:else if active === 'preferencias'}
      <section class="section">
        <header class="s-head">
          <h2>Preferências</h2>
          <p class="muted">
            Padrões de publicação (visibilidade / sensível) e notificações por
            e-mail. As mudanças salvam automaticamente.
          </p>
        </header>
        <PreferencesPanel />
      </section>
    {:else if active === 'importar'}
      <section class="section">
        <header class="s-head">
          <h2>Importar contas do fediverso</h2>
          <p class="muted">
            Cole ou envie um CSV com contas que você já segue em outra
            instância. A gente dispara o Follow pra cada uma.
          </p>
        </header>
        <ImportPanel />
      </section>
    {:else if active === 'filtros'}
      <section class="section">
        <header class="s-head">
          <h2>Filtros de conteúdo</h2>
          <p class="muted">
            Adicione termos que devem esconder publicações do seu feed. Zero
            fanfarra: match simples por substring, case-insensitive.
          </p>
        </header>
        <FiltersPanel />
      </section>
    {:else if active === 'campanha'}
      <section class="section">
        <header class="s-head">
          <h2>Consentimento de campanha</h2>
          <p class="muted">
            Controle quem — se alguém — pode usar seus dados para comunicação de
            campanha. <strong>Padrão: ninguém.</strong> Você autoriza e revoga aqui.
          </p>
        </header>
        <CampaignConsentPanel />
      </section>
    {:else if active === 'lgpd'}
      <section class="section">
        <header class="s-head">
          <h2>Meus dados (LGPD)</h2>
          <p class="muted">
            Direitos do titular conforme Lei 13.709/2018 art. 18: exportar seus
            dados em formato portável, ou excluir sua conta.
          </p>
        </header>
        <LgpdPanel />
      </section>
    {/if}
  </div>
</div>

<style>
  .shell {
    display: block;
  }
  .pane {
    padding-top: var(--sp-5);
  }
  .section {
    display: block;
  }
  .s-head {
    margin-bottom: var(--sp-4);
  }
  .s-head h2 {
    margin: 0 0 var(--sp-1);
    font-size: var(--fs-xl);
    color: var(--text-1);
  }
  .s-head .muted {
    margin: 0;
    color: var(--text-3);
  }
  .sub {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    margin: 0 0 var(--sp-4);
    font-size: var(--fs-lg);
    color: var(--text-1);
  }
  .theme-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--sp-4);
    flex-wrap: wrap;
  }
  .theme-row strong {
    display: block;
    color: var(--text-1);
    margin-bottom: 2px;
  }
  .theme-row .small {
    margin: 0;
    font-size: var(--fs-sm);
  }
  .spacer {
    height: var(--sp-5);
  }
</style>
