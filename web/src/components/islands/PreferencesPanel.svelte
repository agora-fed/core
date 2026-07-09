<script lang="ts">
  // Notificações por e-mail (checkbox por evento) + Padrões de publicação
  // (visibilidade + sensitive). Auto-salva com debounce curto.
  import { onMount } from 'svelte';
  import {
    getMyPreferences,
    patchMyPreferences,
    type MyPreferencesDto,
    type EmailPrefs,
  } from '../../lib/api';
  import { toast } from '../../lib/toasts';
  import Card from '../ui/Card.svelte';

  let loading = $state(true);
  let error = $state<string | null>(null);
  let prefs = $state<MyPreferencesDto | null>(null);
  let saveTimer: ReturnType<typeof setTimeout> | null = null;

  onMount(async () => {
    const res = await getMyPreferences();
    loading = false;
    if (res.success && res.data) {
      prefs = res.data;
    } else {
      error = res.error?.message ?? 'Falha ao carregar preferências.';
    }
  });

  async function persist(patch: Partial<MyPreferencesDto>) {
    if (saveTimer) clearTimeout(saveTimer);
    saveTimer = setTimeout(async () => {
      const res = await patchMyPreferences(patch);
      if (!res.success) {
        toast.error(res.error?.message ?? 'Falha ao salvar.');
      } else {
        toast.success('Preferências salvas.');
      }
    }, 350);
  }

  function setEmail(key: keyof EmailPrefs, value: boolean) {
    if (!prefs) return;
    prefs.email_prefs = { ...prefs.email_prefs, [key]: value };
    persist({ email_prefs: prefs.email_prefs });
  }

  function setVisibility(v: MyPreferencesDto['default_visibility']) {
    if (!prefs) return;
    prefs.default_visibility = v;
    persist({ default_visibility: v });
  }

  function setSensitive(v: boolean) {
    if (!prefs) return;
    prefs.default_sensitive = v;
    persist({ default_sensitive: v });
  }
</script>

{#if loading}
  <p class="muted">Carregando…</p>
{:else if error || !prefs}
  <Card>
    <p class="hint-error">{error ?? 'Preferências indisponíveis.'}</p>
  </Card>
{:else}
  <Card>
    <h3 class="sub">Padrões de publicação</h3>
    <p class="muted small">
      Vale pra <em>novas</em> publicações. Publicações antigas mantêm o que
      você escolheu na hora.
    </p>
    <div class="grid">
      <label class="fld">
        <span>Visibilidade padrão</span>
        <select value={prefs.default_visibility} onchange={(e) => setVisibility((e.currentTarget as HTMLSelectElement).value as MyPreferencesDto['default_visibility'])}>
          <option value="public">Público — no feed federado</option>
          <option value="unlisted">Não listado — só quem tem o link vê</option>
          <option value="followers">Só seguidores</option>
          <option value="direct">Direto (menções apenas)</option>
        </select>
      </label>
      <label class="fld check">
        <input
          type="checkbox"
          checked={prefs.default_sensitive}
          onchange={(e) => setSensitive((e.currentTarget as HTMLInputElement).checked)}
        />
        <span>Marcar publicações como <strong>sensíveis</strong> por padrão</span>
      </label>
    </div>
  </Card>

  <Card>
    <h3 class="sub">Notificações por e-mail</h3>
    <p class="muted small">
      Você continua recebendo notificações in-app (o sininho). Aqui você
      escolhe o que sai também por e-mail.
    </p>
    <div class="check-grid">
      {#each [
        { key: 'mention', label: 'Alguém me mencionou' },
        { key: 'reply', label: 'Alguém respondeu minha publicação' },
        { key: 'favorite', label: 'Alguém favoritou' },
        { key: 'reblog', label: 'Alguém republicou (boost)' },
        { key: 'follow', label: 'Alguém começou a me seguir' },
        { key: 'admin_action', label: 'Ação da moderação sobre minha conta' },
      ] as row (row.key)}
        <label class="check">
          <input
            type="checkbox"
            checked={prefs.email_prefs[row.key as keyof EmailPrefs] !== false}
            onchange={(e) => setEmail(row.key as keyof EmailPrefs, (e.currentTarget as HTMLInputElement).checked)}
          />
          <span>{row.label}</span>
        </label>
      {/each}
    </div>
  </Card>
{/if}

<style>
  .sub {
    margin: 0 0 var(--sp-2);
    font-size: var(--fs-md);
  }
  .small {
    font-size: var(--fs-sm);
    margin-bottom: var(--sp-3);
    line-height: 1.4;
  }
  .grid {
    display: grid;
    gap: var(--sp-3);
    grid-template-columns: 1fr 1fr;
  }
  @media (max-width: 700px) {
    .grid { grid-template-columns: 1fr; }
  }
  .fld {
    display: flex;
    flex-direction: column;
    gap: var(--sp-1);
    font-size: var(--fs-sm);
    font-weight: var(--fw-semibold);
  }
  .fld.check {
    flex-direction: row;
    align-items: center;
    gap: var(--sp-2);
    font-weight: var(--fw-medium);
  }
  .fld select {
    padding: var(--sp-2) var(--sp-3);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-sm);
    background: var(--surface-1);
    color: var(--text-1);
    font: inherit;
    font-size: var(--fs-sm);
  }
  .check-grid {
    display: grid;
    gap: var(--sp-2);
  }
  .check {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    font-size: var(--fs-sm);
    padding: var(--sp-2);
    border-radius: var(--r-sm);
    cursor: pointer;
  }
  .check:hover {
    background: var(--surface-2);
  }
  .hint-error {
    color: var(--danger, #b91c1c);
    font-size: 0.9rem;
  }
</style>
