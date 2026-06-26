// CPF (Cadastro de Pessoas Físicas) helpers — client-side check-digit validation.
// The CPF has 11 digits; the last two are verification digits computed mod 11.

/** Strip everything but digits. */
export function onlyDigits(value: string): string {
  return value.replace(/\D+/g, '');
}

/** Format raw digits as `000.000.000-00` progressively (for inputs). */
export function formatCpf(value: string): string {
  const d = onlyDigits(value).slice(0, 11);
  const parts = [d.slice(0, 3), d.slice(3, 6), d.slice(6, 9)].filter(Boolean);
  const tail = d.slice(9, 11);
  let out = parts.join('.');
  if (tail) out += `-${tail}`;
  return out;
}

function checkDigit(digits: number[], factorStart: number): number {
  const sum = digits.reduce((acc, n, i) => acc + n * (factorStart - i), 0);
  const rest = (sum * 10) % 11;
  return rest === 10 ? 0 : rest;
}

/** Validate a CPF by its two check digits. Rejects known invalid repdigits. */
export function isValidCpf(value: string): boolean {
  const d = onlyDigits(value);
  if (d.length !== 11) return false;
  // Reject sequences like 00000000000, 11111111111, … (valid mod-11 but fake).
  if (/^(\d)\1{10}$/.test(d)) return false;

  const nums = d.split('').map(Number);
  const d1 = checkDigit(nums.slice(0, 9), 10);
  if (d1 !== nums[9]) return false;
  const d2 = checkDigit(nums.slice(0, 10), 11);
  return d2 === nums[10];
}
