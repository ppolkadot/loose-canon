use super::*;

const MARK: &str = "[removido]";

fn assert_removed(text: &str, secret: &str) {
    let out = redact(text);
    assert!(!out.contains(secret), "{secret:?} survived in {out:?}");
    assert!(out.contains(MARK), "no marker in {out:?}");
}

#[test]
fn e_mail_addresses_are_removed() {
    assert_removed(
        "Contato: maria.silva+casa@exemplo.com.br hoje",
        "maria.silva",
    );
    assert_removed("fale com JOAO@Imobiliaria.COM", "JOAO");
}

#[test]
fn links_are_removed_with_their_path_and_query() {
    assert_removed(
        "veja https://www.exemplo.com.br/imovel/123?utm=a&b=c agora",
        "exemplo",
    );
    assert_removed("veja http://exemplo.com/x", "exemplo");
    assert_removed("mais fotos em www.exemplo.com.br/fotos", "exemplo");
    assert_removed("site exemplo.com.br/casa-3-quartos", "exemplo");
    assert_removed("site exemplo.com.br/casa-3-quartos", "casa-3-quartos");
    assert_removed(
        "veja https://www.exemplo.com.br/imovel/123?utm=a&b=c agora",
        "utm",
    );
}

#[test]
fn social_handles_are_removed() {
    assert_removed("siga @imoveis_goiania no instagram", "goiania");
    assert_removed("siga @imoveis_goiania no instagram", "imoveis");
    assert_removed("siga @a.b.c no instagram", ".c");
}

#[test]
fn brazilian_phone_numbers_are_removed_in_every_common_format() {
    for phone in [
        "(62) 99999-8888",
        "(62)99999-8888",
        "62 99999-8888",
        "62999998888",
        "+55 62 99999-8888",
        "+5562999998888",
        "55 62 99999 8888",
        "99999-8888",
        "9 9999-8888",
        "(62) 3333-4444",
        "3333-4444",
        "62 3333 4444",
        "0800 123 4567",
    ] {
        let out = redact(&format!("Ligue {phone} e agende"));
        assert!(out.contains(MARK), "{phone:?} -> {out:?}");
        assert!(
            !out.chars().any(|c| c.is_ascii_digit()),
            "a digit of {phone:?} survived: {out:?}"
        );
    }
}

#[test]
fn a_phone_next_to_words_like_whatsapp_loses_only_the_number() {
    let out = redact("Chame no WhatsApp (62) 98888-7777 para visitar");
    assert!(out.contains("WhatsApp"), "{out:?}");
    assert!(out.contains("para visitar"), "{out:?}");
    assert!(!out.contains("98888"), "{out:?}");
}

#[test]
fn registration_numbers_are_removed() {
    assert_removed("CPF 123.456.789-09 do proprietário", "123.456.789-09");
    assert_removed("CNPJ 12.345.678/0001-95", "12.345.678/0001-95");
    assert_removed("Corretor CRECI 12345-F GO", "2345");
    assert_removed("Corretor CRECI 12345-F GO", "-F");
    assert_removed("creci: 98765", "8765");
    assert_removed("CRECI-GO 54321", "4321");
}

#[test]
fn prices_areas_and_room_counts_are_left_untouched() {
    for text in [
        "Valor total R$ 450.000,00",
        "Valor R$ 450.000",
        "por R$450.000",
        "R$ 12000000",
        "R$ 1.250.000,00 à vista",
        "180 m² de área construída, lote 360 m²",
        "lote 360m2 com 12x30",
        "3 quartos, 1 suíte, 2 vagas",
        "Setor Bueno, 2ª etapa, quadra 12 lote 4",
        "ágio de R$ 80 mil, parcelas de R$ 2.350,00",
        "IPTU R$ 1.234,56 ano 2026",
    ] {
        assert_eq!(redact(text), text, "this is not contact data");
    }
}

#[test]
fn text_without_contact_data_is_returned_unchanged() {
    let text = "Casa ampla com quintal, piscina e churrasqueira, próxima ao parque.";
    assert_eq!(redact(text), text);
}

#[test]
fn redacting_twice_changes_nothing_more() {
    let text = "Ligue (62) 99999-8888 ou escreva a a@b.com, veja www.x.com.br e @user";
    let once = redact(text);
    assert_eq!(redact(&once), once);
}

#[test]
fn several_items_in_one_text_are_all_removed() {
    let out = redact("a@b.com | (62) 3333-4444 | https://x.com.br/y | @user | CRECI 1234");
    for secret in ["a@b.com", "3333", "x.com.br", "user", "1234"] {
        assert!(!out.contains(secret), "{secret:?} survived in {out:?}");
    }
}

#[test]
fn an_excerpt_collapses_whitespace_and_redacts_before_cutting() {
    let out = excerpt("Casa   boa\n\n  ligue (62) 99999-8888\tagora", 200);
    assert_eq!(out, format!("Casa boa ligue {MARK} agora"));
}

#[test]
fn an_excerpt_never_exceeds_the_limit_and_cuts_at_a_word() {
    let out = excerpt("um dois três quatro cinco seis sete", 14);
    assert!(out.chars().count() <= 14, "{out:?}");
    assert!(out.ends_with('…'), "{out:?}");
    assert!(out.starts_with("um dois"), "{out:?}");
    let body = out.trim_end_matches('…');
    assert!(
        "um dois três quatro cinco seis sete".starts_with(body),
        "cut inside a word? {out:?}"
    );
    assert!(
        !"um dois três quatro cinco seis sete"[body.len()..].starts_with(|c: char| c != ' '),
        "cut inside a word? {out:?}"
    );
}

#[test]
fn a_cut_that_lands_right_before_a_space_keeps_the_whole_word() {
    assert_eq!(excerpt("aaa bbb ccc", 8), "aaa bbb…");
    assert_eq!(
        excerpt("aaa bbb ccc", 9),
        "aaa bbb…",
        "no room for the next word"
    );
}

#[test]
fn a_short_text_is_not_cut_and_gets_no_ellipsis() {
    assert_eq!(excerpt("Casa boa", 100), "Casa boa");
    assert_eq!(excerpt("Casa boa", 8), "Casa boa", "exactly the limit fits");
}

#[test]
fn cutting_never_splits_a_multi_byte_character() {
    let out = excerpt("ação ação ação ação ação", 11);
    assert!(out.chars().count() <= 11, "{out:?}");
    assert!(out.is_char_boundary(out.len()));
}

#[test]
fn a_single_word_longer_than_the_limit_is_still_cut_to_the_limit() {
    let out = excerpt(&"a".repeat(50), 10);
    assert_eq!(out.chars().count(), 10, "{out:?}");
    assert!(out.ends_with('…'));
}

#[test]
fn an_empty_text_and_a_zero_limit_give_an_empty_excerpt() {
    assert_eq!(excerpt("", 10), "");
    assert_eq!(excerpt("   \n ", 10), "");
    assert_eq!(excerpt("Casa", 0), "");
}

#[test]
fn phone_numbers_with_unusual_separators_are_removed() {
    for (text, digits) in [
        ("zap 62-99999-8888", "99999"),
        ("zap 62.99999.8888", "99999"),
        ("zap (62)9 9999-8888", "9999"),
        ("zap 62 9 9999 8888", "8888"),
        ("zap 6299999-8888", "99999"),
        ("zap: +55 (62) 99999-8888.", "99999"),
        ("tel 62 3333.4444", "3333"),
    ] {
        let out = redact(text);
        assert!(!out.contains(digits), "{text:?} -> {out:?}");
    }
}

#[test]
fn a_price_and_a_phone_in_one_text_are_told_apart() {
    let out = redact("Valor R$ 450.000,00 ou R$ 12000000 total. Ligue 62 99999-8888 agora");
    assert!(out.contains("R$ 450.000,00"), "{out:?}");
    assert!(out.contains("R$ 12000000"), "{out:?}");
    assert!(!out.contains("99999"), "{out:?}");
}

#[test]
fn removing_a_handle_keeps_the_text_around_it_intact() {
    assert_eq!(redact("siga @user hoje"), format!("siga {MARK} hoje"));
    assert_eq!(redact("(@user)"), format!("({MARK})"));
    assert_eq!(redact("@user"), MARK);
}
