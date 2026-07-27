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
  import PhonePanel from './PhonePanel.svelte';
  import TotpPanel from './TotpPanel.svelte';
  import InterestsPanel from './InterestsPanel.svelte';
  import FiltersPanel from './FiltersPanel.svelte';
  import PreferencesPanel from './PreferencesPanel.svelte';
  import ImportPanel from './ImportPanel.svelte';

  type TabId =
    | 'perfil'
    | 'identidade'
    | 'aparencia'
    | 'preferencias'
    | 'seguranca'
    | 'fediverso'
    | 'dados';

  const tabs: { id: TabId; label: string }[] = [
    { id: 'perfil', label: 'Perfil' },
    { id: 'identidade', label: 'Identidade' },
    { id: 'aparencia', label: 'Aparência' },
    { id: 'preferencias', label: 'Preferências' },
    { id: 'seguranca', label: 'Segurança & acesso' },
    { id: 'fediverso', label: 'Fediverso' },
    { id: 'dados', label: 'Meus dados' },
  ];

  // Compat: hashes antigos (#lgpd, #2fa, #campanha…) caem no grupo novo.
  const HASH_ALIAS: Record<string, TabId> = {
    telefone: 'seguranca',
    aplicativos: 'seguranca',
    lgpd: 'dados',
    campanha: 'dados',
    filtros: 'fediverso',
    importar: 'fediverso',
  };

  let active = $state<TabId>('perfil');

  onMount(() => {
    const h = window.location.hash.replace('#', '');
    if (tabs.some((t) => t.id === h)) active = h as TabId;
    else if (HASH_ALIAS[h]) active = HASH_ALIAS[h];
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

        <div class="spacer"></div>
        <h3 class="grp">Meus interesses</h3>
        <p class="muted small">
          Áreas (baseadas na estrutura ministerial) sobre as quais você quer receber atualizações.
        </p>
        <InterestsPanel />
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
    {:else if active === 'seguranca'}
      <section class="section">
        <header class="s-head">
          <h2>Segurança &amp; acesso</h2>
          <p class="muted">Senha, sessões ativas, verificação em duas etapas e apps conectados.</p>
        </header>

        <Card>
          <h3 class="sub"><Icon name="lock" size={18} /> Trocar senha</h3>
          <ChangePasswordForm />
        </Card>
        <div class="spacer"></div>
        <SessionsList />

        <div class="spacer"></div>
        <h3 class="grp">Verificação em duas etapas (2FA)</h3>
        <p class="muted small">
          <strong>TOTP</strong> (app autenticador) é o recomendado; telefone/SMS é alternativa
          (não recomendada).
        </p>
        <TotpPanel />
        <PhonePanel />

        <div class="spacer"></div>
        <h3 class="grp">Aplicativos conectados</h3>
        <p class="muted small">
          Clientes Mastodon e integrações via OAuth. Desconecte um app para revogar o acesso.
        </p>
        <AuthorizedApps />
      </section>
    {:else if active === 'fediverso'}
      <section class="section">
        <header class="s-head">
          <h2>Fediverso</h2>
          <p class="muted">
            Descubra perfis em outras instâncias, importe quem você já segue e filtre o feed.
          </p>
        </header>

        <h3 class="grp">Descobrir perfis</h3>
        <p class="muted small">Busque em outras instâncias (Mastodon, Pleroma, Misskey…) e siga.</p>
        <FediverseSearch />

        <div class="spacer"></div>
        <h3 class="grp">Importar contas</h3>
        <p class="muted small">
          Cole ou envie um CSV de contas que você já segue em outra instância — disparamos o Follow.
        </p>
        <ImportPanel />

        <div class="spacer"></div>
        <h3 class="grp">Filtros de conteúdo</h3>
        <p class="muted small">Termos que escondem publicações do feed (substring, case-insensitive).</p>
        <FiltersPanel />
      </section>
    {:else if active === 'dados'}
      <section class="section">
        <header class="s-head">
          <h2>Meus dados</h2>
          <p class="muted">Consentimento de campanha e seus direitos de titular (LGPD).</p>
        </header>

        <h3 class="grp">Consentimento de campanha</h3>
        <p class="muted small">
          Controle quem — se alguém — pode usar seus dados para campanha. <strong>Padrão: ninguém.</strong>
        </p>
        <CampaignConsentPanel />

        <div class="spacer"></div>
        <h3 class="grp">Direitos LGPD (art. 18)</h3>
        <p class="muted small">Exporte seus dados em formato portável ou exclua sua conta.</p>
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
  /* Cabeçalho de subgrupo dentro de uma aba agrupada (2FA, Filtros, LGPD…). */
  .grp {
    margin: 0 0 var(--sp-1);
    padding-top: var(--sp-2);
    border-top: 1px solid var(--border-subtle);
    font-size: var(--fs-md);
    color: var(--text-1);
  }
  .grp + .muted {
    margin-top: 0;
    margin-bottom: var(--sp-3);
  }
</style>
