export const SITE_NAME: string = import.meta.env.PUBLIC_SITE_NAME || 'AGORA';
export const SITE_HOST: string = new URL(import.meta.env.SITE ?? 'http://localhost').host;
