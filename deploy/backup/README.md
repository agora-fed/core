# Backup do banco (0.42.0)

Cumpre a promessa de `web/src/pages/privacidade.astro`: **backup diário
criptografado, retenção 30 dias**. Antes disto NÃO havia rotina de backup —
o Postgres roda em host único (ADR-0002), então este é o único ponto de
recuperação contra perda/corrupção de dados.

## Componentes
- `dsoc-backup.sh` → `/usr/local/bin/dsoc-backup.sh` — faz `pg_dump -Fc` e cifra
  com AES-256 (openssl, chave em `/etc/dsoc/backup.key`). Plaintext nunca toca o
  disco (dump→cifra num pipe). Retenção 30 dias.
- `dsoc-backup.service` + `dsoc-backup.timer` → `/etc/systemd/system/` — roda
  diariamente às 03:12 UTC, como usuário `postgres`, catch-up se a VM estava off.
- Saída: `/var/backups/dsoc/democracia_social-<YYYYMMDD-HHMMSS>.dump.enc`.

## Instalação (na VM de prod, como popsolutions com sudo)
```sh
# chave de cifra (uma vez), só o postgres lê:
sudo mkdir -p /etc/dsoc && sudo openssl rand -base64 48 | sudo tee /etc/dsoc/backup.key >/dev/null
sudo chown postgres:postgres /etc/dsoc/backup.key && sudo chmod 600 /etc/dsoc/backup.key
sudo mkdir -p /var/backups/dsoc && sudo chown postgres:postgres /var/backups/dsoc

sudo cp dsoc-backup.sh /usr/local/bin/ && sudo chmod 755 /usr/local/bin/dsoc-backup.sh
sudo cp dsoc-backup.service dsoc-backup.timer /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now dsoc-backup.timer
sudo systemctl start dsoc-backup.service   # roda um agora
```

## Restaurar
```sh
openssl enc -d -aes-256-cbc -pbkdf2 -pass file:/etc/dsoc/backup.key \
  -in /var/backups/dsoc/democracia_social-<ts>.dump.enc \
  | sudo -u postgres pg_restore -d democracia_social --clean --if-exists
```

## Guardar a chave FORA da VM
`/etc/dsoc/backup.key` é o que decifra tudo. Se a VM for perdida, o backup é
inútil sem a chave — copie-a para um cofre offline (o operador faz isto à mão;
nunca commitar a chave).

## Follow-up (não coberto aqui)
- **Off-site**: hoje o backup fica NA MESMA VM. Um `mc mirror` para outro
  bucket/host, ou `scp` para outra máquina, fecharia o risco de perda física.
- Disco da VM está apertado (~83% em 2026-07-24). Monitorar `/var/backups/dsoc`.
