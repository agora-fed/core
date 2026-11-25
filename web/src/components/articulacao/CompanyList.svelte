<script lang="ts">
  // The companies of one person (Receita Federal, diogenes ADR-0010). The role is the
  // Receita text; the page never calls a partner an owner.
  import { brl, cnpjBase, companyDetail, moreCompanies, namesakesText, type CompanyView } from '../../lib/articulacao';

  let { companies, total = companies.length }: { companies: CompanyView[]; total?: number } = $props();
</script>

{#if companies.length}
  <ul class="companies">
    {#each companies as c (c.cnpj_basico)}
      <li>
        <span class="name">{c.name}</span>
        <span class="muted">CNPJ {cnpjBase(c.cnpj_basico)}{c.capital_cents ? ` · capital ${brl(c.capital_cents)}` : ''}</span>
        <span class="detail">{companyDetail(c)}</span>
        {#if c.namesakes}<span class="detail warn">Atenção: {namesakesText(c.namesakes)}; o CPF parcial coincide com o do doador.</span>{/if}
      </li>
    {/each}
    {#if moreCompanies(companies.length, total)}
      <li class="muted">{moreCompanies(companies.length, total)}</li>
    {/if}
  </ul>
{/if}

<style>
  .companies {
    list-style: none;
    margin: var(--sp-1) 0 0;
    padding: 0 0 0 var(--sp-3);
    border-left: 2px solid var(--border-subtle);
    display: grid;
    gap: var(--sp-1);
    font-size: var(--fs-sm);
  }
  .companies li {
    display: flex;
    flex-wrap: wrap;
    column-gap: var(--sp-2);
  }
  .name {
    font-weight: var(--fw-medium);
    overflow-wrap: anywhere;
  }
  .detail {
    flex-basis: 100%;
    color: var(--text-2);
  }
  .warn {
    font-style: italic;
  }
</style>
