# 62imoveis.com.br

Evaluated: **2026-10-27**. Decision: **do not integrate** (terms prohibit automated collection).

## Requests made

| URL | Result |
|---|---|
| `https://www.62imoveis.com.br/robots.txt` | HTTP 200, 331 bytes, SHA-256 `60438f43…d312e` |
| `https://www.62imoveis.com.br/politica-de-privacidade` | HTTP 200 (privacy policy; no automation clauses; says it is part of the Terms of Use) |
| `https://www.62imoveis.com.br/termo-uso` | HTTP 200, "Última atualização: 20/02/2026", HTML SHA-256 `2a16f0e2…2d31` |

Server: Cloudflare. No block was encountered for these three documents.

## `robots.txt` (verbatim)

```
User-agent: *
Disallow: /imovel/impressao/
Disallow: /imovel/termo-de-visita/
Disallow: /favoritos/
Disallow: /visitas/
Disallow: /propostas/
Disallow: /conta/
Disallow: /filtros/
Disallow: /imoveis-vistos
Disallow: /busca/ajax/
Disallow: /imoveis-visitados/
Sitemap: https://www.62imoveis.com.br/sitemap_62imoveis.xml
```

Listing pages are not disallowed and a sitemap is published. **This is not authorization**; the
terms below govern.

## Terms of Use (relevant wording, Portuguese)

> O USUÁRIO poderá acessar, utilizar e imprimir materiais deste portal para seu uso pessoal e
> não-comercial, sendo expressamente vedado, contudo, copiar, distribuir, transmitir, exibir,
> reproduzir, modificar, publicar, licenciar, criar trabalho derivado, colocar e/ou usar em outra
> página da Internet, transferir ou vender qualquer informação, software, solução tecnológica, lista
> de USUÁRIOS, produtos ou serviços obtidos no PORTAL, para fins comerciais. **Esta proibição
> expressamente inclui, mas não se limita, à prática de screen scraping ou database scraping para
> obter listas de USUÁRIOS ou outras informações, para uso comercial ou não.**

Also, in the Intellectual Property section: reproduction, distribution and transmission of the
portal's materials (including databases and images) "não são permitidas sem o expresso consentimento
por escrito" of the portal's owner.

## Assessment

* Scraping "para uso comercial ou não" is expressly prohibited, so a personal-use intention does
  not cure it.
* No API, feed or data licence for third parties was found on the pages reviewed.
* Reproducing photos and text into a local database also falls under the written-consent clause.

## To reopen

Written authorization from the portal's owner for this use, or a published API/feed with terms that
allow it. Re-read the terms at that time.

## Not verified

Whether a partner/API programme exists (none was found; none was searched beyond the three pages
above). Whether the terms are enforced against personal, low-rate use (irrelevant to the decision:
this project follows the published terms).
