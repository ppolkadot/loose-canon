# ZAP Imóveis (zapimoveis.com.br)

Evaluated: **2026-10-27**. Decision: **do not integrate** (terms prohibit robot-based reproduction
and reuse of content; edge blocks automated clients; no third-party read access).

## Requests made

| URL | Result |
|---|---|
| `https://www.zapimoveis.com.br/robots.txt` | **HTTP 403** (Cloudflare). Not worked around. |
| `https://www.zapimoveis.com.br/termos-de-uso/` | HTTP 404 (a guessed path; the returned page's footer gave the real terms link, no further site request) |
| `https://ajuda.zapimoveis.com.br/s/article/termos-e-condicoes-gerais-de-uso-zap` | HTTP 200 ("Atualização em 17 de Agosto de 2026"; body links to the PDF below) |
| `https://d337qhvnm4f82f.cloudfront.net/media/images/OLX_TCG_14082026_f61a2e6cc9.pdf` | HTTP 200, 12,683,079 bytes, SHA-256 `ccb75e56…eea7` (file name `OLX_TCG_14082026`) |
| `https://developers.grupozap.com/…` | See `olx.md` (advertiser integrations only) |

## Terms (Grupo OLX T&C linked from ZAP's terms page; quoted by wording)

> Utilizar textos, imagens, anúncios e qualquer outro elemento incluído ou disponível nas
> Plataformas do grupo OLX para sua posterior inclusão em quaisquer locais diversos das Plataformas
> grupo OLX;

> **Utilizar robôs nos websites em que utilizar o grupo OLX para reproduzir, exibir, copiar,
> transformar, modificar, desmontar, fazer engenharia reversa, distribuir, alugar, fornecer, colocar
> à disposição do público**, através de qualquer forma de comunicação pública, qualquer dos elementos
> protegidos.

> Você não usará e não autorizará terceiros a utilizarem-se de quaisquer meios automatizados
> incluindo, sem limitação, robôs, scripts ou spiders, para interferir ou tentar interferir no
> funcionamento adequado do grupo OLX e de seus websites;

The PDF is the group-wide advertiser-oriented T&C and its extracted text is column-scrambled, so
the exact section numbers for each clause were not established; the wording above is verbatim.

## Assessment

* Using robots to reproduce/copy platform elements and reusing ads/texts/images elsewhere is
  prohibited without authorization. A local database of listing text and photos is such a use.
* No third-party read API or feed exists on the group's developer portal.
* Automated clients are refused at the edge (HTTP 403 for `robots.txt`).

## To reopen

Written authorization from Grupo OLX or a licensed data agreement; re-read the then-current terms.

## Not verified

The ZAP-specific `privacidade.zapimoveis.com.br` and quality-policy pages were not read. Whether a
commercial data product exists (not searched).
