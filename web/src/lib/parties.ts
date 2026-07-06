// Helpers para a seção Organizações (partidos). O catálogo é derivado dos mandatos, então estas
// funções operam sobre a sigla (mandate.party) tal como vem da base oficial (case preservado:
// "PCdoB", "PT", "PSOL"…).

/** URL-safe slug de uma sigla de partido. "PCdoB" -> "pcdob". Usado nas rotas /organizacoes/{slug}. */
export function partySlug(sigla: string): string {
  return sigla
    .normalize('NFD')
    .replace(/[̀-ͯ]/g, '')
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '');
}

// Cor institucional aproximada por sigla — só estética (barra lateral + crest). Fallback neutro
// para partidos fora da lista. Não é fonte de verdade sobre nada.
const PARTY_COLORS: Record<string, string> = {
  pt: '#c4122e',
  pcdob: '#a3161f',
  psol: '#e63329',
  pv: '#2e8b3d',
  rede: '#00a19a',
  psb: '#e30613',
  pdt: '#153d8a',
  pcb: '#b30000',
  up: '#d32f2f',
};

/** Cor institucional aproximada da sigla (hex). Neutra quando desconhecida. */
export function partyColor(sigla: string): string {
  return PARTY_COLORS[partySlug(sigla)] ?? '#5b6472';
}
