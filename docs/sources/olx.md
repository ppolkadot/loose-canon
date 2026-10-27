# OLX (olx.com.br)

Evaluated: **2026-10-27**. Decision: **do not integrate** (terms prohibit crawling; edge blocks
automated clients; no third-party read access).

## Requests made

| URL | Result |
|---|---|
| `https://www.olx.com.br/robots.txt` | **HTTP 403** (Cloudflare). Not worked around. |
| `https://ajuda.olx.com.br/s/article/termos-e-condicoes-de-uso` | HTTP 200 (help-centre article; the body is a short intro that links to the full terms PDF) |
| `https://ajuda.olx.com.br/public/Termos_e_Condicoes_de_Uso_OLX_27022025_727855f932.pdf` | HTTP 200, 1,160,677 bytes, SHA-256 `121d54ef…7697` ("Atualização em 27 de Fevereiro de 2025") |
| `https://developers.grupozap.com/` and `/feeds/connection_guideline.html` | HTTP 200 (see below) |

The main site refuses an automated client even for `robots.txt`, so its crawl policy could not be
read at all.

## Terms (section "Propriedade Intelectual", clause listing what the user may not do)

Lead-in: "Você não poderá, exceto com autorização prévia e expressa dos titulares:" followed by,
among others (quoted by wording; the PDF extraction scrambles column order):

> **Realizar web crawling no Site**, reproduzir, exibir, copiar, transformar, modificar, desmontar,
> realizar engenharia reversa, distribuir, alugar, fornecer, colocar à disposição do público, através
> de qualquer modalidade de comunicação pública, qualquer dos elementos protegidos.

> Utilizar de textos, imagens, anúncios e qualquer outro elemento incluído ou disponível no Site para
> sua posterior inclusão em quaisquer veículos alheios ao Site sem a autorização prévia e por escrito
> do Grupo OLX.

> Manipular quaisquer dispositivos técnicos estabelecidos pelo Grupo OLX ou por terceiros para
> proteção de seus direitos […]

## Grupo OLX developer portal (verified)

`developers.grupozap.com` ("Portal de integração Grupo OLX") covers *advertiser* integrations: feeds,
webhooks, "Aluguel Digital", Leads API, events, lead manager. The feed guide states that the group's
own probe polls the advertiser's XML ("a cada 12 horas"; `User-Agent: VivaRealBot/1.0`). It is about
**sending** listings to the group. It does not offer third parties search or read access, which
confirms the claim in `docs/phase-2-design.md`.

## Assessment

* Crawling is prohibited without prior express authorization.
* There is no third-party read API or feed.
* Automated clients are refused at the edge.

## To reopen

Prior written authorization from Grupo OLX (or a licensed data agreement). Re-read the then-current
terms.

## Not verified

Whether a commercial data-licensing product exists (not searched; web search was unavailable).
