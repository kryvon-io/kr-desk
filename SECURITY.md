# Política de segurança

O KR Desk é um software de acesso remoto; falhas de segurança podem ter impacto sério.

## Como reportar

**Não abra uma issue pública.** Use o recurso de relatório privado do GitHub:
aba **Security → Report a vulnerability** neste repositório.

Inclua, se possível: versão afetada, passos para reproduzir e impacto esperado.

## Versões suportadas

Enquanto o projeto estiver em 0.x, apenas a versão mais recente recebe correções.

## Princípios

- Nenhuma sessão remota começa sem consentimento explícito no computador controlado.
- Mídia e dados trafegam cifrados (DTLS-SRTP); a sinalização usa apenas TLS.
- Segredos de infraestrutura nunca ficam no repositório (somente nos painéis da Vercel/GitHub).
