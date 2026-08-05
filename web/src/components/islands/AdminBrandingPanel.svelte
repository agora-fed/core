<script lang="ts">
  // Runtime branding panel (Odoo-style): site name, tagline, logo/favicon and
  // the semantic accent tokens — stored server-side (org_branding, 0674) and
  // applied by the shell on every page load. Colors preview live while editing.
  import { onMount } from 'svelte';
  import {
    adminGetBranding,
    adminPutBranding,
    type BrandingDto,
  } from '../../lib/api';
  import { toast } from '../../lib/toasts';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';

  /** Tokens the server allowlists — keep in sync with ALLOWED_COLOR_TOKENS. */
  type TokenDef = { key: string; label: string; hint: string };
  const TOKEN_GROUPS: { title: string; tokens: TokenDef[] }[] = [
    {
      title: 'Marca',
      tokens: [
        { key: 'accent', label: 'Cor primária', hint: 'botões, links, destaques' },
        { key: 'accent-strong', label: 'Primária escura', hint: 'hover, ênfase' },
        { key: 'accent-soft', label: 'Primária suave', hint: 'fundos, chips' },
        { key: 'accent-contrast', label: 'Contraste', hint: 'texto sobre a primária' },
      ],
    },
    {
      title: 'Superfícies',
      tokens: [
        { key: 'surface-0', label: 'Fundo da página', hint: '' },
        { key: 'surface-1', label: 'Cartões', hint: '' },
        { key: 'surface-2', label: 'Superfície aninhada', hint: '' },
        { key: 'surface-3', label: 'Hover / campos', hint: '' },
        { key: 'surface-inverse', label: 'Superfície inversa', hint: 'rodapé, hero' },
      ],
    },
    {
      title: 'Texto',
      tokens: [
        { key: 'text-1', label: 'Texto primário', hint: '' },
        { key: 'text-2', label: 'Texto secundário', hint: '' },
        { key: 'text-3', label: 'Texto suave', hint: 'legendas' },
        { key: 'text-inverse', label: 'Texto inverso', hint: 'sobre fundo escuro' },
      ],
    },
    {
      title: 'Bordas',
      tokens: [
        { key: 'border-subtle', label: 'Borda sutil', hint: '' },
        { key: 'border-strong', label: 'Borda forte', hint: '' },
      ],
    },
    {
      title: 'Estados',
      tokens: [
        { key: 'danger', label: 'Erro', hint: '' },
        { key: 'danger-soft', label: 'Erro (fundo)', hint: '' },
        { key: 'warning', label: 'Alerta', hint: '' },
        { key: 'warning-soft', label: 'Alerta (fundo)', hint: '' },
        { key: 'info', label: 'Informação', hint: '' },
        { key: 'info-soft', label: 'Informação (fundo)', hint: '' },
        { key: 'success', label: 'Sucesso', hint: '' },
        { key: 'success-soft', label: 'Sucesso (fundo)', hint: '' },
      ],
    },
  ];
  const TOKENS: TokenDef[] = TOKEN_GROUPS.flatMap((g) => g.tokens);
  const HEX_RE = /^#(?:[0-9a-fA-F]{3}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$/;

  let loading = $state(true);
  let saving = $state(false);
  let error = $state<string | null>(null);

  let siteName = $state('');
  let tagline = $state('');
  let logoUrl = $state('');
  let faviconUrl = $state('');
  let colors = $state<Record<string, string>>({});

  function fromDto(b: BrandingDto) {
    siteName = b.site_name ?? '';
    tagline = b.tagline ?? '';
    logoUrl = b.logo_url ?? '';
    faviconUrl = b.favicon_url ?? '';
    colors = { ...(b.colors ?? {}) };
  }

  function toDto(): BrandingDto {
    const clean: Record<string, string> = {};
    for (const t of TOKENS) {
      const v = (colors[t.key] ?? '').trim();
      if (v) clean[t.key] = v;
    }
    return {
      site_name: siteName.trim() || null,
      tagline: tagline.trim() || null,
      logo_url: logoUrl.trim() || null,
      favicon_url: faviconUrl.trim() || null,
      colors: clean,
    };
  }

  /** Live preview: mirror the edited tokens onto the page immediately. */
  function preview() {
    const root = document.documentElement;
    for (const t of TOKENS) {
      const v = (colors[t.key] ?? '').trim();
      if (HEX_RE.test(v)) root.style.setProperty(`--${t.key}`, v);
      else root.style.removeProperty(`--${t.key}`);
    }
  }

  const invalidColor = (key: string): boolean => {
    const v = (colors[key] ?? '').trim();
    return v.length > 0 && !HEX_RE.test(v);
  };

  async function reload() {
    loading = true;
    error = null;
    const res = await adminGetBranding();
    if (res.success && res.data) fromDto(res.data);
    else error = res.error?.message ?? 'Falha ao carregar identidade visual.';
    loading = false;
  }

  async function save() {
    if (TOKENS.some((t) => invalidColor(t.key))) {
      toast('Cores devem ser #rgb, #rrggbb ou #rrggbbaa.', 'error');
      return;
    }
    saving = true;
    const res = await adminPutBranding(toDto());
    saving = false;
    if (res.success && res.data) {
      fromDto(res.data);
      preview();
      toast('Identidade visual salva. Recarregue para ver em todo o site.', 'success');
    } else {
      toast(res.error?.message ?? 'Falha ao salvar.', 'error');
    }
  }

  onMount(reload);
</script>

{#if loading}
  <p class="muted">Carregando…</p>
{:else if error}
  <p class="error">{error}</p>
{:else}
  <div class="grid">
    <Card>
      <h2>Identidade</h2>
      <label>
        Nome do site
        <input type="text" bind:value={siteName} maxlength="80" placeholder="DemocraciaBR" />
      </label>
      <label>
        Slogan
        <input type="text" bind:value={tagline} maxlength="200"
          placeholder="Participação com consequência" />
      </label>
      <label>
        URL do logo
        <input type="url" bind:value={logoUrl} maxlength="500" placeholder="/media/logo.png" />
      </label>
      <label>
        URL do favicon
        <input type="url" bind:value={faviconUrl} maxlength="500" placeholder="/favicon-512.png" />
      </label>
      {#if logoUrl.trim()}
        <div class="logo-preview">
          <img src={logoUrl} alt="Prévia do logo" height="40" />
        </div>
      {/if}
    </Card>

    <Card>
      <h2>Cores</h2>
      <p class="muted">
        Tokens semânticos do design system — o servidor só aceita esta lista
        (nenhum CSS arbitrário). Deixe em branco para manter o tema padrão.
      </p>
      {#each TOKEN_GROUPS as group (group.title)}
        <h3>{group.title}</h3>
        {#each group.tokens as t (t.key)}
          <label class="color-row" class:invalid={invalidColor(t.key)}>
            <span class="color-label">
              {t.label}
              {#if t.hint}<small class="muted">({t.hint})</small>{/if}
            </span>
            <span class="color-inputs">
              <input
                type="color"
                value={HEX_RE.test(colors[t.key] ?? '') ? colors[t.key] : '#15803d'}
                oninput={(e) => {
                  colors[t.key] = (e.currentTarget as HTMLInputElement).value;
                  preview();
                }}
              />
              <input
                type="text"
                placeholder="#15803d"
                bind:value={colors[t.key]}
                oninput={preview}
              />
            </span>
          </label>
        {/each}
      {/each}
    </Card>
  </div>

  <div class="actions">
    <Button onclick={save} disabled={saving}>
      {saving ? 'Salvando…' : 'Salvar identidade visual'}
    </Button>
  </div>
{/if}

<style>
  .grid {
    display: grid;
    gap: var(--sp-4);
    grid-template-columns: repeat(auto-fit, minmax(320px, 1fr));
  }
  h2 {
    margin: 0 0 var(--sp-3);
    font-size: var(--fs-lg);
  }
  label {
    display: block;
    margin-bottom: var(--sp-3);
    font-size: var(--fs-sm);
  }
  input[type='text'],
  input[type='url'] {
    display: block;
    width: 100%;
    margin-top: var(--sp-1);
  }
  .color-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--sp-3);
  }
  .color-row.invalid input[type='text'] {
    border-color: var(--danger, #dc2626);
  }
  .color-inputs {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
  }
  .color-inputs input[type='text'] {
    width: 7.5rem;
    margin-top: 0;
  }
  .logo-preview {
    padding: var(--sp-2);
    border: 1px dashed var(--border-1, #ccc);
    border-radius: 6px;
    display: inline-block;
  }
  .actions {
    margin-top: var(--sp-5);
  }
  .muted {
    color: var(--text-2, #666);
  }
  .error {
    color: var(--danger, #dc2626);
  }
</style>
