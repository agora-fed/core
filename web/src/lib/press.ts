// Conteúdo da sala de imprensa (/imprensa), trilíngue PT · EN · ES. Fonte única
// consumida por PressKit.astro e pela geração dos PDFs. Redação HONESTA: projeto
// em desenvolvimento ativo, sem número de usuários, sem superprometer (ver
// docs/PLANO-ESTRATEGICO-2026.md §1.2). Citação atribuída à cooperativa.

export type Lang = 'pt' | 'en' | 'es';

export interface Fact {
  label: string;
  value: string;
}

export interface PressContent {
  lang: Lang;
  metaTitle: string;
  metaDescription: string;
  /** rótulo do alternador (nome do idioma nele próprio) */
  langName: string;
  kicker: string;
  headline: string;
  lede: string;
  body: string[];
  quote: string;
  quoteAttribution: string;
  closing: string;
  boilerplateHeading: string;
  boilerplateShort: string;
  boilerplateLong: string;
  factSheetHeading: string;
  facts: Fact[];
  mediaKitHeading: string;
  mediaKitIntro: string;
  brandHeading: string;
  paletteHeading: string;
  typographyHeading: string;
  typographyText: string;
  downloadsHeading: string;
  contactHeading: string;
  contactText: string;
  contactCta: string;
  dl: {
    logo: string;
    emblem: string;
    og: string;
    manual: string;
    releasePdf: string;
    zip: string;
  };
}

const pt: PressContent = {
  lang: 'pt',
  metaTitle: 'Imprensa',
  metaDescription:
    'Sala de imprensa da DemocraciaBR: press release, mídia kit, logos e fatos sobre a plataforma cívica soberana e aberta da PopSolutions Software & Comunicação LTDA.',
  langName: 'Português',
  kicker: 'Para divulgação imediata',
  headline:
    'DemocraciaBR: plataforma soberana e aberta transforma demanda popular em cobrança com prazo aos mandatos brasileiros',
  lede: 'A PopSolutions Software & Comunicação LTDA — uma empresa administrada cooperativamente — coloca no ar a DemocraciaBR, uma infraestrutura pública, federada e de código aberto que conecta a cidadania a toda a cadeia política do país — de vereadores à Presidência — sob uma tese simples: participação sem consequência é teatro.',
  body: [
    'Diferente de uma rede social ou de um portal de petições, a DemocraciaBR fecha um loop de consequência: o cidadão propõe, a comunidade apoia e vota, e ao cruzar um patamar a demanda é encaminhada ao gabinete responsável com um relógio de resposta público. Se o mandato responde, fica registrado; se silencia, o silêncio também vira registro — imutável e auditável — e alimenta o placar público daquele parlamentar.',
    'A plataforma fala ActivityPub, o mesmo protocolo do Mastodon: perfis, publicações e enquetes federam com o fediverso, e aplicativos como Tusky e Elk conseguem se conectar à instância. Já estão indexados cerca de 70 mil mandatos reais a partir de dados abertos oficiais (Câmara, Senado, prefeituras e câmaras municipais), cada um com um placar público de promessas e respostas.',
    'A DemocraciaBR é soberana e sem fins lucrativos: roda em servidor no Brasil, não tem investidores, e todo o código é aberto sob licença AGPL-3.0 com Cláusula de Contrato Social — qualquer pessoa pode auditar, hospedar a sua própria instância e federar. A manutenção é da PopSolutions Software & Comunicação LTDA, uma empresa administrada cooperativamente por desenvolvedores e pesquisadores dedicados a infraestrutura democrática auditável.',
    'O projeto está em desenvolvimento ativo, com evolução contínua rumo ao ciclo eleitoral de 2026.',
  ],
  quote:
    'Nossa aposta é devolver à população uma ferramenta pública que o poder não controla e não pode ignorar em silêncio.',
  quoteAttribution: 'PopSolutions Software & Comunicação LTDA',
  closing: 'Conheça em democracia.social.br.',
  boilerplateHeading: 'Sobre a DemocraciaBR',
  boilerplateShort:
    'A DemocraciaBR é uma infraestrutura pública, soberana e de código aberto que conecta a cidadania aos mandatos brasileiros — participação com consequência: cada demanda a um mandato tem prazo público de resposta, e o silêncio vira registro. Federa via ActivityPub. Mantida pela PopSolutions Software & Comunicação LTDA, uma empresa administrada cooperativamente, sem investidores e sem fins lucrativos. Licença AGPL-3.0.',
  boilerplateLong:
    'A DemocraciaBR converte demanda cidadã em accountability público, com prazo e permanente. Uma proposta apoiada e votada pela comunidade é encaminhada ao gabinete responsável com um relógio de resposta visível a todos; a resposta — ou o silêncio — fica registrada de forma imutável e alimenta o placar público de cada mandato, hoje já indexado a partir de cerca de 70 mil registros oficiais. Por falar ActivityPub, a plataforma interopera com o fediverso e com clientes Mastodon. É soberana (roda no Brasil, IPv6-first), aberta (AGPL-3.0 + Contrato Social) e mantida pela PopSolutions Software & Comunicação LTDA (empresa administrada cooperativamente), sem investidores e sem fins lucrativos.',
  factSheetHeading: 'Fatos rápidos',
  facts: [
    { label: 'O que é', value: 'Infraestrutura cívica pública, federada (ActivityPub) e de código aberto' },
    { label: 'Tese', value: 'Participação sem consequência é teatro; o silêncio vira registro' },
    { label: 'Mantenedora', value: 'PopSolutions Software & Comunicação LTDA — empresa administrada cooperativamente (sem investidores, sem fins lucrativos)' },
    { label: 'Licença', value: 'AGPL-3.0-or-later + Cláusula de Contrato Social' },
    { label: 'Site', value: 'democracia.social.br (servidor no Brasil, IPv6-first)' },
    { label: 'Código', value: 'git.pop.coop/brasil/democracia-social' },
    { label: 'Já no ar', value: 'Federação ActivityPub/Mastodon; loop propor→votar→prazo→placar; fóruns, propostas e consultas; papéis e moderação; cadastro com verificação de CPF; ~70 mil mandatos indexados; LGPD (exportar/apagar dados)' },
    { label: 'Em desenvolvimento', value: 'Consenso semântico pleno, prazo proporcional ao eleitorado, app nativo, ciclo eleitoral 2026' },
    { label: 'Tecnologia', value: 'Rust (Axum/Tokio); PostgreSQL 17 + pgvector; Astro + Svelte; Kubernetes (k3s) IPv6' },
  ],
  mediaKitHeading: 'Mídia kit',
  mediaKitIntro:
    'Logos, cores, tipografia e textos prontos para uso editorial. Baixe os arquivos abaixo ou o kit completo.',
  brandHeading: 'Marca',
  paletteHeading: 'Cores',
  typographyHeading: 'Tipografia',
  typographyText: 'Poppins (400 / 500 / 600 / 700). Fonte de código aberto (Open Font License).',
  downloadsHeading: 'Downloads',
  contactHeading: 'Contato de imprensa',
  contactText:
    'Para entrevistas, dados ou material adicional, fale com a gente pelo formulário — selecione o setor Imprensa.',
  contactCta: 'Falar com a imprensa',
  dl: {
    logo: 'Logo (PNG)',
    emblem: 'Emblema (PNG)',
    og: 'Card social (PNG)',
    manual: 'Manual de identidade (PNG)',
    releasePdf: 'Press release (PDF)',
    zip: 'Baixar kit completo (ZIP)',
  },
};

const en: PressContent = {
  lang: 'en',
  metaTitle: 'Press',
  metaDescription:
    "DemocraciaBR press room: press release, media kit, logos and facts about PopSolutions Software & Comunicação LTDA's sovereign, open-source civic platform.",
  langName: 'English',
  kicker: 'For immediate release',
  headline:
    'DemocraciaBR: a sovereign, open platform turns public demand into time-bound accountability for Brazilian officials',
  lede: 'PopSolutions Software & Comunicação LTDA — a cooperatively-run company — launches DemocraciaBR, a public, federated and open-source infrastructure that connects citizens to the country’s entire political chain — from city councillors to the Presidency — under a simple thesis: participation without consequence is theater.',
  body: [
    'Unlike a social network or a petition portal, DemocraciaBR closes a consequence loop: a citizen proposes, the community backs and votes, and once a threshold is crossed the demand is forwarded to the responsible official’s office with a public response clock. If the official answers, it is recorded; if they stay silent, the silence is also recorded — immutable and auditable — and feeds that official’s public scorecard.',
    'The platform speaks ActivityPub, the same protocol as Mastodon: profiles, posts and polls federate with the fediverse, and apps such as Tusky and Elk can connect to the instance. Around 70,000 real mandates are already indexed from official open data (federal Chamber, Senate, city halls and municipal councils), each with a public scorecard of promises and answers.',
    'DemocraciaBR is sovereign and non-profit: it runs on a server in Brazil, has no investors, and all code is open under the AGPL-3.0 license with a Social Contract clause — anyone can audit it, host their own instance and federate. It is stewarded by PopSolutions Software & Comunicação LTDA, a cooperatively-run company of developers and researchers dedicated to auditable democratic infrastructure.',
    'The project is under active development, evolving toward the 2026 election cycle.',
  ],
  quote:
    'Our bet is to give people back a public tool that power does not control and cannot ignore in silence.',
  quoteAttribution: 'PopSolutions Software & Comunicação LTDA',
  closing: 'Learn more at democracia.social.br.',
  boilerplateHeading: 'About DemocraciaBR',
  boilerplateShort:
    'DemocraciaBR is a public, sovereign and open-source infrastructure connecting citizens to Brazilian officials — participation with consequence: every demand addressed to an office has a public response deadline, and silence becomes a record. It federates via ActivityPub. Stewarded by PopSolutions Software & Comunicação LTDA, a cooperatively-run company, with no investors and non-profit. AGPL-3.0 licensed.',
  boilerplateLong:
    'DemocraciaBR turns citizen demand into public, time-bound and permanent accountability. A proposal backed and voted by the community is forwarded to the responsible office with a response clock visible to everyone; the answer — or the silence — is recorded immutably and feeds each official’s public scorecard, already indexed from roughly 70,000 official records. By speaking ActivityPub, the platform interoperates with the fediverse and Mastodon clients. It is sovereign (runs in Brazil, IPv6-first), open (AGPL-3.0 + Social Contract) and stewarded by PopSolutions Software & Comunicação LTDA (a cooperatively-run company) — no investors, non-profit.',
  factSheetHeading: 'Fast facts',
  facts: [
    { label: 'What it is', value: 'Public, federated (ActivityPub) and open-source civic infrastructure' },
    { label: 'Thesis', value: 'Participation without consequence is theater; silence becomes a record' },
    { label: 'Steward', value: 'PopSolutions Software & Comunicação LTDA — cooperatively-run company (no investors, non-profit)' },
    { label: 'License', value: 'AGPL-3.0-or-later + Social Contract clause' },
    { label: 'Website', value: 'democracia.social.br (server in Brazil, IPv6-first)' },
    { label: 'Code', value: 'git.pop.coop/brasil/democracia-social' },
    { label: 'Already live', value: 'ActivityPub/Mastodon federation; propose→vote→deadline→scorecard loop; forums, proposals and consultations; roles and moderation; sign-up with CPF verification; ~70,000 indexed mandates; LGPD (data export/delete)' },
    { label: 'In development', value: 'Full semantic consensus, electorate-proportional thresholds, native app, 2026 election cycle' },
    { label: 'Technology', value: 'Rust (Axum/Tokio); PostgreSQL 17 + pgvector; Astro + Svelte; Kubernetes (k3s) over IPv6' },
  ],
  mediaKitHeading: 'Media kit',
  mediaKitIntro:
    'Logos, colors, typography and ready-to-use copy for editorial use. Download the files below or the full kit.',
  brandHeading: 'Brand',
  paletteHeading: 'Colors',
  typographyHeading: 'Typography',
  typographyText: 'Poppins (400 / 500 / 600 / 700). Open-source font (Open Font License).',
  downloadsHeading: 'Downloads',
  contactHeading: 'Press contact',
  contactText:
    'For interviews, data or additional material, reach us through the form — select the Press sector.',
  contactCta: 'Contact the press desk',
  dl: {
    logo: 'Logo (PNG)',
    emblem: 'Emblem (PNG)',
    og: 'Social card (PNG)',
    manual: 'Brand manual (PNG)',
    releasePdf: 'Press release (PDF)',
    zip: 'Download full kit (ZIP)',
  },
};

const es: PressContent = {
  lang: 'es',
  metaTitle: 'Prensa',
  metaDescription:
    'Sala de prensa de DemocraciaBR: comunicado, kit de prensa, logos y datos sobre la plataforma cívica soberana y abierta de PopSolutions Software & Comunicação LTDA.',
  langName: 'Español',
  kicker: 'Para difusión inmediata',
  headline:
    'DemocraciaBR: una plataforma soberana y abierta convierte la demanda popular en rendición de cuentas con plazo para los cargos brasileños',
  lede: 'PopSolutions Software & Comunicação LTDA — una empresa administrada cooperativamente — lanza DemocraciaBR, una infraestructura pública, federada y de código abierto que conecta a la ciudadanía con toda la cadena política del país — de concejales a la Presidencia — bajo una tesis simple: la participación sin consecuencia es teatro.',
  body: [
    'A diferencia de una red social o un portal de peticiones, DemocraciaBR cierra un circuito de consecuencia: la persona propone, la comunidad respalda y vota, y al cruzar un umbral la demanda se envía al despacho responsable con un reloj de respuesta público. Si el cargo responde, queda registrado; si guarda silencio, el silencio también queda registrado — inmutable y auditable — y alimenta el marcador público de ese representante.',
    'La plataforma habla ActivityPub, el mismo protocolo de Mastodon: perfiles, publicaciones y encuestas federan con el fediverso, y aplicaciones como Tusky y Elk pueden conectarse a la instancia. Ya están indexados unos 70 mil mandatos reales a partir de datos abiertos oficiales (Cámara, Senado, alcaldías y concejos municipales), cada uno con un marcador público de promesas y respuestas.',
    'DemocraciaBR es soberana y sin fines de lucro: funciona en un servidor en Brasil, no tiene inversores, y todo el código es abierto bajo licencia AGPL-3.0 con Cláusula de Contrato Social — cualquiera puede auditarlo, alojar su propia instancia y federar. La mantiene PopSolutions Software & Comunicação LTDA, una empresa administrada cooperativamente por desarrolladores e investigadores dedicados a la infraestructura democrática auditable.',
    'El proyecto está en desarrollo activo, con evolución continua hacia el ciclo electoral de 2026.',
  ],
  quote:
    'Nuestra apuesta es devolverle a la gente una herramienta pública que el poder no controla y no puede ignorar en silencio.',
  quoteAttribution: 'PopSolutions Software & Comunicação LTDA',
  closing: 'Conócela en democracia.social.br.',
  boilerplateHeading: 'Sobre DemocraciaBR',
  boilerplateShort:
    'DemocraciaBR es una infraestructura pública, soberana y de código abierto que conecta a la ciudadanía con los cargos brasileños — participación con consecuencia: cada demanda a un despacho tiene un plazo público de respuesta, y el silencio se vuelve registro. Federa vía ActivityPub. Mantenida por PopSolutions Software & Comunicação LTDA, una empresa administrada cooperativamente, sin inversores y sin fines de lucro. Licencia AGPL-3.0.',
  boilerplateLong:
    'DemocraciaBR convierte la demanda ciudadana en rendición de cuentas pública, con plazo y permanente. Una propuesta respaldada y votada por la comunidad se envía al despacho responsable con un reloj de respuesta visible para todos; la respuesta — o el silencio — queda registrada de forma inmutable y alimenta el marcador público de cada cargo, ya indexado a partir de unos 70 mil registros oficiales. Al hablar ActivityPub, la plataforma interopera con el fediverso y con clientes Mastodon. Es soberana (funciona en Brasil, IPv6-first), abierta (AGPL-3.0 + Contrato Social) y mantenida por PopSolutions Software & Comunicação LTDA (empresa administrada cooperativamente) — sin inversores, sin fines de lucro.',
  factSheetHeading: 'Datos rápidos',
  facts: [
    { label: 'Qué es', value: 'Infraestructura cívica pública, federada (ActivityPub) y de código abierto' },
    { label: 'Tesis', value: 'La participación sin consecuencia es teatro; el silencio se vuelve registro' },
    { label: 'Mantenedora', value: 'PopSolutions Software & Comunicação LTDA — empresa administrada cooperativamente (sin inversores, sin fines de lucro)' },
    { label: 'Licencia', value: 'AGPL-3.0-or-later + Cláusula de Contrato Social' },
    { label: 'Sitio', value: 'democracia.social.br (servidor en Brasil, IPv6-first)' },
    { label: 'Código', value: 'git.pop.coop/brasil/democracia-social' },
    { label: 'Ya en línea', value: 'Federación ActivityPub/Mastodon; circuito proponer→votar→plazo→marcador; foros, propuestas y consultas; roles y moderación; registro con verificación de CPF; ~70 mil mandatos indexados; LGPD (exportar/borrar datos)' },
    { label: 'En desarrollo', value: 'Consenso semántico pleno, plazo proporcional al electorado, app nativa, ciclo electoral 2026' },
    { label: 'Tecnología', value: 'Rust (Axum/Tokio); PostgreSQL 17 + pgvector; Astro + Svelte; Kubernetes (k3s) sobre IPv6' },
  ],
  mediaKitHeading: 'Kit de prensa',
  mediaKitIntro:
    'Logos, colores, tipografía y textos listos para uso editorial. Descarga los archivos o el kit completo.',
  brandHeading: 'Marca',
  paletteHeading: 'Colores',
  typographyHeading: 'Tipografía',
  typographyText: 'Poppins (400 / 500 / 600 / 700). Fuente de código abierto (Open Font License).',
  downloadsHeading: 'Descargas',
  contactHeading: 'Contacto de prensa',
  contactText:
    'Para entrevistas, datos o material adicional, escríbenos por el formulario — selecciona el sector Prensa.',
  contactCta: 'Contactar a prensa',
  dl: {
    logo: 'Logo (PNG)',
    emblem: 'Emblema (PNG)',
    og: 'Tarjeta social (PNG)',
    manual: 'Manual de identidad (PNG)',
    releasePdf: 'Comunicado (PDF)',
    zip: 'Descargar kit completo (ZIP)',
  },
};

export const PRESS: Record<Lang, PressContent> = { pt, en, es };

/** Paleta oficial exibida na página (de global.css / manual de identidade). */
export const PALETTE: { name: string; hex: string }[] = [
  { name: 'Verde', hex: '#15803d' },
  { name: 'Azul', hex: '#1d4ed8' },
  { name: 'Vermelho', hex: '#e84c3d' },
  { name: 'Âmbar', hex: '#f4c20d' },
  { name: 'Navy', hex: '#0f172a' },
];
