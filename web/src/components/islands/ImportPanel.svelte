<script lang="ts">
  // Tab Importar em /configuracoes. Aceita CSV Mastodon (uma URL/handle por
  // linha) ou textarea colando linhas soltas. Chama /me/bulk_follow.
  import { bulkFollow, type BulkFollowResultDto } from '../../lib/api';
  import { toast } from '../../lib/toasts';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';

  let text = $state('');
  let busy = $state(false);
  let result = $state<BulkFollowResultDto | null>(null);

  function parseLines(s: string): string[] {
    // Mastodon "following_accounts.csv" tem header "Account address"; ignoramos.
    return s
      .split(/\r?\n/)
      .map((line) => line.split(',')[0].trim())
      .filter((line) => line.length > 0 && line.toLowerCase() !== 'account address' && !line.startsWith('#'));
  }

  function onFilePicked(e: Event) {
    const input = e.currentTarget as HTMLInputElement;
    const f = input.files?.[0];
    if (!f) return;
    const reader = new FileReader();
    reader.onload = () => { text = String(reader.result || ''); };
    reader.readAsText(f);
  }

  async function onSubmit(e: SubmitEvent) {
    e.preventDefault();
    const entries = parseLines(text);
    if (entries.length === 0) {
      toast.error('Nenhuma entrada válida.');
      return;
    }
    busy = true;
    result = null;
    const res = await bulkFollow(entries);
    busy = false;
    if (res.success && res.data) {
      result = res.data;
      toast.success(`Enviou ${res.data.followed} pedidos de seguir.`);
    } else {
      toast.error(res.error?.message ?? 'Falha ao importar.');
    }
  }
</script>

<Card>
  <p class="muted small">
    Cole uma lista com um <code>@usuario@instancia</code> ou URL do actor por
    linha. Também aceita CSV exportado do Mastodon (arquivo
    <code>following_accounts.csv</code>). Máximo 200 por chamada.
  </p>
  <form onsubmit={onSubmit} class="form">
    <label class="fld wide">
      <span>Cole as contas ou envie o CSV</span>
      <textarea
        bind:value={text}
        rows="10"
        placeholder="@zedirceu@masto.social&#10;@jandira@bolha.us&#10;https://mastodon.social/users/Gargron"
      ></textarea>
    </label>
    <label class="fld">
      <span>Ou envie CSV</span>
      <input type="file" accept=".csv,text/csv,text/plain" onchange={onFilePicked} />
    </label>
    <Button type="submit" variant="primary" loading={busy} disabled={!text.trim()}>
      Seguir todos
    </Button>
  </form>
</Card>

{#if result}
  <Card>
    <h3 class="sub">Resultado</h3>
    <ul class="stats">
      <li><strong>{result.total}</strong> entradas</li>
      <li><strong>{result.followed}</strong> novos seguidos</li>
      <li><strong>{result.already}</strong> já seguia</li>
      <li><strong>{result.failed}</strong> falharam</li>
    </ul>
    {#if result.errors.length > 0}
      <details>
        <summary>Ver erros ({result.errors.length})</summary>
        <ul class="errs">
          {#each result.errors as e}
            <li><code>{e}</code></li>
          {/each}
        </ul>
      </details>
    {/if}
  </Card>
{/if}

<style>
  .small { font-size: var(--fs-sm); margin-bottom: var(--sp-3); }
  code { font-family: ui-monospace, SFMono-Regular, monospace; background: var(--surface-2); padding: 1px 5px; border-radius: 4px; }
  .form {
    display: grid;
    grid-template-columns: 1fr 300px auto;
    gap: var(--sp-3);
    align-items: end;
  }
  .fld { display: flex; flex-direction: column; gap: var(--sp-1); font-size: var(--fs-sm); font-weight: var(--fw-semibold); }
  .fld.wide { grid-column: 1 / -1; }
  .fld input, .fld textarea {
    padding: var(--sp-2) var(--sp-3);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-sm);
    background: var(--surface-1);
    color: var(--text-1);
    font: inherit; font-size: var(--fs-sm);
  }
  .fld textarea { resize: vertical; font-family: ui-monospace, SFMono-Regular, monospace; }
  @media (max-width: 800px) { .form { grid-template-columns: 1fr; } }
  .sub { margin: 0 0 var(--sp-2); font-size: var(--fs-md); }
  .stats { list-style: none; padding: 0; margin: 0 0 var(--sp-3); display: flex; gap: var(--sp-4); flex-wrap: wrap; font-size: var(--fs-sm); }
  .errs { max-height: 200px; overflow: auto; margin: var(--sp-2) 0 0; padding-left: 1.2rem; }
</style>
