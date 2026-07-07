<script lang="ts">
  // Visual catalog for the design system primitives. Not shipped to indexes
  // (BaseLayout is called with noindex=true on the /design route). Renders
  // each primitive with a few representative states so a designer can eyeball
  // dark/light + interactive behavior without spinning up Storybook.
  import Button from '../ui/Button.svelte';
  import Card from '../ui/Card.svelte';
  import Input from '../ui/Input.svelte';
  import Textarea from '../ui/Textarea.svelte';
  import Badge from '../ui/Badge.svelte';
  import Avatar from '../ui/Avatar.svelte';
  import Icon from '../ui/Icon.svelte';
  import Modal from '../ui/Modal.svelte';
  import Menu from '../ui/Menu.svelte';
  import Tooltip from '../ui/Tooltip.svelte';
  import Alert from '../ui/Alert.svelte';
  import Skeleton from '../ui/Skeleton.svelte';
  import Spinner from '../ui/Spinner.svelte';
  import EmptyState from '../ui/EmptyState.svelte';
  import ErrorState from '../ui/ErrorState.svelte';
  import Tabs from '../ui/Tabs.svelte';
  import Chip from '../ui/Chip.svelte';
  import Switch from '../ui/Switch.svelte';
  import Toast from '../ui/Toast.svelte';
  import { toast } from '../../lib/toasts';

  let modalOpen = $state(false);
  let name = $state('');
  let bio = $state('');
  let active = $state('mandatos');
  let sel = $state(new Set(['PT', 'PSOL']));
  let checked = $state(true);

  function toggle(k: string) {
    sel = new Set(sel);
    if (sel.has(k)) sel.delete(k);
    else sel.add(k);
  }

  const iconNames = [
    'home',
    'feed',
    'search',
    'plus',
    'bell',
    'chat',
    'message',
    'calendar',
    'hashtag',
    'at',
    'globe',
    'users',
    'profile',
    'heart',
    'heart-fill',
    'boost',
    'bookmark',
    'bookmark-fill',
    'reply',
    'share',
    'pin',
    'more',
    'edit',
    'trash',
    'upload',
    'camera',
    'video',
    'poll',
    'cw',
    'settings',
    'moon',
    'sun',
    'chevron-down',
    'chevron-up',
    'chevron-left',
    'chevron-right',
    'arrow-left',
    'arrow-right',
    'x',
    'check',
    'alert',
    'info',
    'filter',
    'sort',
    'eye',
    'eye-off',
    'lock',
    'unlock',
    'gavel',
    'ballot',
    'mic',
    'chart',
    'verified',
    'party',
    'mandate',
    'sla-pending',
    'sla-answered',
    'sla-acted',
    'sla-ignored',
    'menu',
    'external',
    'copy',
    'link',
  ];
</script>

<div class="doc">
  <Toast />
  <header class="head">
    <h1>Design System v1</h1>
    <p>
      Catálogo vivo dos primitivos da 0.17.0. Alterne o tema pelo cabeçalho
      para ver dark/light. Todos os componentes consomem tokens semânticos
      (<code>--surface-*</code>, <code>--text-*</code>, <code>--accent</code>).
    </p>
  </header>

  <section>
    <h2>Tokens</h2>
    <div class="tok-grid">
      <div class="tok"><span class="sw" style="background:var(--surface-0)"></span>surface-0</div>
      <div class="tok"><span class="sw" style="background:var(--surface-1)"></span>surface-1</div>
      <div class="tok"><span class="sw" style="background:var(--surface-2)"></span>surface-2</div>
      <div class="tok"><span class="sw" style="background:var(--surface-3)"></span>surface-3</div>
      <div class="tok"><span class="sw" style="background:var(--accent)"></span>accent</div>
      <div class="tok"><span class="sw" style="background:var(--accent-strong)"></span>accent-strong</div>
      <div class="tok"><span class="sw" style="background:var(--accent-soft)"></span>accent-soft</div>
      <div class="tok"><span class="sw" style="background:var(--success)"></span>success</div>
      <div class="tok"><span class="sw" style="background:var(--warning)"></span>warning</div>
      <div class="tok"><span class="sw" style="background:var(--danger)"></span>danger</div>
      <div class="tok"><span class="sw" style="background:var(--info)"></span>info</div>
    </div>
  </section>

  <section>
    <h2>Typography</h2>
    <Card padding="lg">
      <h1 style="margin:0">H1 · clamp 32→52</h1>
      <h2 style="margin:0">H2 · clamp 24→40</h2>
      <h3 style="margin:0">H3 · 20</h3>
      <p style="font-size:var(--fs-lg)">Body large — 18 · Poppins</p>
      <p style="font-size:var(--fs-md)">Body — 17 (default)</p>
      <p style="font-size:var(--fs-sm)">Small — 14</p>
      <p style="font-size:var(--fs-xs); color:var(--text-3)">Meta — 12 muted</p>
    </Card>
  </section>

  <section>
    <h2>Buttons</h2>
    <div class="row">
      <Button variant="primary">Primary</Button>
      <Button variant="secondary">Secondary</Button>
      <Button variant="ghost">Ghost</Button>
      <Button variant="subtle">Subtle</Button>
      <Button variant="danger">Danger</Button>
      <Button variant="primary" disabled>Disabled</Button>
      <Button variant="primary" loading>Loading</Button>
    </div>
    <div class="row">
      <Button size="sm">Small</Button>
      <Button size="base">Base</Button>
      <Button size="lg">Large</Button>
    </div>
    <div class="row">
      <Button variant="secondary"><Icon name="plus" size={16} />Nova proposta</Button>
      <Button variant="ghost"><Icon name="search" size={16} />Buscar</Button>
    </div>
  </section>

  <section>
    <h2>Badges</h2>
    <div class="row">
      <Badge tone="neutral">Neutro</Badge>
      <Badge tone="accent">Accent</Badge>
      <Badge tone="pending"><Icon name="sla-pending" size={12} />Pendente</Badge>
      <Badge tone="answered"><Icon name="sla-answered" size={12} />Respondida</Badge>
      <Badge tone="acted"><Icon name="sla-acted" size={12} />Compromisso</Badge>
      <Badge tone="ignored"><Icon name="sla-ignored" size={12} />Silêncio público</Badge>
      <Badge tone="info" outline>Info outline</Badge>
    </div>
  </section>

  <section>
    <h2>Avatares</h2>
    <div class="row">
      <Avatar size="xs" name="M" />
      <Avatar size="sm" name="Marina" />
      <Avatar size="base" name="Carlos" />
      <Avatar size="lg" name="Ana Paula" />
      <Avatar size="xl" name="Rodrigo" ring />
    </div>
  </section>

  <section>
    <h2>Inputs</h2>
    <div class="grid-forms">
      <Card>
        <Input
          label="Nome público"
          placeholder="ex.: Marina Silva"
          bind:value={name}
          hint="Aparece no seu perfil."
        />
        <Input
          label="E-mail"
          type="email"
          placeholder="voce@exemplo.br"
          leading={leading}
          required
        />
        <Input
          label="Senha"
          type="password"
          error="Precisa de pelo menos 12 caracteres."
        />
        <Textarea
          label="Biografia"
          rows={4}
          maxlength={280}
          bind:value={bio}
          placeholder="Uma linha sobre você…"
        />

        {#snippet leading()}
          <Icon name="at" size={16} />
        {/snippet}
      </Card>
      <Card>
        <p style="font-size:var(--fs-sm); color:var(--text-2)">
          <strong>Switch:</strong>
          <Switch bind:checked label="Perfil público" />
        </p>
        <p style="font-size:var(--fs-sm); color:var(--text-2)">
          <strong>Chips (filtro por partido):</strong>
        </p>
        <div class="row">
          {#each ['PT', 'PSOL', 'REDE', 'PDT', 'MDB', 'NOVO', 'PL'] as p}
            <Chip selected={sel.has(p)} onclick={() => toggle(p)}>{p}</Chip>
          {/each}
        </div>
      </Card>
    </div>
  </section>

  <section>
    <h2>Feedback</h2>
    <div class="grid-fb">
      <Alert tone="info" title="Prazo do parlamentar">
        Este mandato tem 72 horas para responder.
      </Alert>
      <Alert tone="success" title="Compromisso registrado">
        A resposta foi publicada no scorecard.
      </Alert>
      <Alert tone="warning" title="Silêncio próximo">
        Faltam menos de 12 horas para expirar.
      </Alert>
      <Alert tone="danger" title="Silêncio público registrado">
        O mandato não respondeu no prazo. Registro imutável.
      </Alert>
    </div>
    <Card>
      <div class="row">
        <Button variant="ghost" onclick={() => toast.success('Salvo com sucesso')}>Toast · success</Button>
        <Button variant="ghost" onclick={() => toast.info('Perfil sincronizado')}>Toast · info</Button>
        <Button variant="ghost" onclick={() => toast.warning('SLA prestes a expirar')}>Toast · warning</Button>
        <Button variant="ghost" onclick={() => toast.error('Falha ao enviar', 'Erro')}>Toast · error</Button>
      </div>
    </Card>
  </section>

  <section>
    <h2>Loading & vazio</h2>
    <div class="grid-fb">
      <Card>
        <p><Spinner /> Carregando dados…</p>
        <p><Skeleton lines={3} /></p>
        <p><Skeleton variant="block" height="6rem" /></p>
      </Card>
      <Card padding="none">
        <EmptyState
          icon="feed"
          title="Sem publicações ainda"
          description="Siga alguém no fediverso ou publique sua primeira nota."
          action={action1}
        />
        {#snippet action1()}
          <Button variant="primary">Publicar</Button>
        {/snippet}
      </Card>
      <Card padding="none">
        <ErrorState retry={() => toast.info('Tentativa disparada')} />
      </Card>
    </div>
  </section>

  <section>
    <h2>Overlays</h2>
    <div class="row">
      <Button onclick={() => (modalOpen = true)}>Abrir modal</Button>
      <Menu label="Ações">
        {#snippet trigger({ toggle: t, open })}
          <Button variant="secondary" onclick={t}>
            Ações <Icon name={open ? 'chevron-up' : 'chevron-down'} size={14} />
          </Button>
        {/snippet}
        {#snippet items()}
          <a href="#"><Icon name="edit" size={14} />Editar</a>
          <a href="#"><Icon name="pin" size={14} />Fixar</a>
          <a href="#"><Icon name="share" size={14} />Compartilhar</a>
          <button type="button"><Icon name="trash" size={14} />Excluir</button>
        {/snippet}
      </Menu>
      <Tooltip text="Este mandato foi verificado por e-mail oficial">
        <Badge tone="answered"><Icon name="verified" size={12} />Verificado</Badge>
      </Tooltip>
    </div>
  </section>

  <section>
    <h2>Tabs</h2>
    <Card padding="none">
      <div style="padding: 0 var(--sp-5)">
        <Tabs
          tabs={[
            { id: 'mandatos', label: 'Mandatos', count: 594 },
            { id: 'partidos', label: 'Partidos', count: 22 },
            { id: 'propostas', label: 'Propostas', count: 128 },
            { id: 'debates', label: 'Debates' },
          ]}
          bind:active
        >
          {#snippet children(id)}
            <div style="padding: var(--sp-5) 0">
              Aba ativa: <strong>{id}</strong>
            </div>
          {/snippet}
        </Tabs>
      </div>
    </Card>
  </section>

  <section>
    <h2>Ícones ({iconNames.length})</h2>
    <div class="icons">
      {#each iconNames as n}
        <div class="icon-cell" title={n}>
          <Icon name={n} size={22} />
          <span>{n}</span>
        </div>
      {/each}
    </div>
  </section>

  <Modal bind:open={modalOpen} title="Registrar compromisso público">
    <p style="margin: 0 0 var(--sp-3); color: var(--text-2); font-size: var(--fs-sm)">
      Ao publicar este compromisso, ele fica anexado ao seu placar público e ao
      mandato correspondente. Uma vez registrado, o compromisso é imutável.
    </p>
    <Textarea
      label="Descrição do compromisso"
      rows={4}
      placeholder="Ex.: apresentar projeto até 30/09"
    />
    {#snippet footer()}
      <Button variant="ghost" onclick={() => (modalOpen = false)}>Cancelar</Button>
      <Button variant="primary" onclick={() => (modalOpen = false)}>Registrar</Button>
    {/snippet}
  </Modal>
</div>

<style>
  .doc {
    max-width: 60rem;
    margin: 0 auto;
    padding: var(--sp-6) var(--sp-5);
    display: flex;
    flex-direction: column;
    gap: var(--sp-8);
  }
  .head p {
    color: var(--text-2);
    font-size: var(--fs-lg);
    max-width: 42rem;
  }
  section h2 {
    font-size: var(--fs-2xl);
    margin-bottom: var(--sp-4);
  }
  code {
    background: var(--surface-2);
    padding: 1px 6px;
    border-radius: var(--r-xs);
    font-size: 0.9em;
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    gap: var(--sp-3);
    align-items: center;
    margin-bottom: var(--sp-3);
  }
  .grid-forms,
  .grid-fb {
    display: grid;
    gap: var(--sp-4);
  }
  @media (min-width: 720px) {
    .grid-forms {
      grid-template-columns: 1fr 1fr;
    }
    .grid-fb {
      grid-template-columns: 1fr 1fr;
    }
  }
  .tok-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
    gap: var(--sp-2);
  }
  .tok {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-2) var(--sp-3);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-sm);
    background: var(--surface-1);
    font-size: var(--fs-xs);
    font-family: monospace;
    color: var(--text-2);
  }
  .sw {
    width: 20px;
    height: 20px;
    border-radius: 4px;
    border: 1px solid var(--border-subtle);
    flex-shrink: 0;
  }
  .icons {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(96px, 1fr));
    gap: var(--sp-2);
  }
  .icon-cell {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--sp-1);
    padding: var(--sp-3) var(--sp-2);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-sm);
    background: var(--surface-1);
    color: var(--text-2);
    font-size: 10px;
    text-align: center;
    word-break: break-all;
  }
</style>
