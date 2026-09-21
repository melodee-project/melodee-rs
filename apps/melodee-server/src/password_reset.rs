//! The deliberately narrow browser endpoint used to complete a password reset.
//!
//! This is not a listener web application.  It is a self-contained security
//! page whose only remote calls are same-origin calls to the native reset API.
//! The token is read from the initial URL, removed from browser history before
//! any API call, and never copied into the returned HTML or browser storage.

use axum::{
    Router,
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use uuid::Uuid;

#[derive(Clone, Copy)]
struct ResetCopy {
    code: &'static str,
    direction: &'static str,
    title: &'static str,
    instructions: &'static str,
    password: &'static str,
    confirm_password: &'static str,
    submit: &'static str,
    checking: &'static str,
    missing: &'static str,
    invalid: &'static str,
    too_short: &'static str,
    mismatch: &'static str,
    rate_limited: &'static str,
    unavailable: &'static str,
    success: &'static str,
    success_detail: &'static str,
}

const EN_US: ResetCopy = ResetCopy {
    code: "en-US",
    direction: "ltr",
    title: "Reset your password",
    instructions: "Choose a new password for your Melodee account.",
    password: "New password",
    confirm_password: "Confirm new password",
    submit: "Set new password",
    checking: "Checking this reset link…",
    missing: "This reset link is missing its token. Request a new password-reset email.",
    invalid: "This reset link is invalid, expired, or has already been used. Request a new password-reset email.",
    too_short: "Your password must be at least 8 characters.",
    mismatch: "The passwords do not match.",
    rate_limited: "Too many attempts. Please wait and try again later.",
    unavailable: "The reset service is unavailable. Please try again later.",
    success: "Your password has been reset.",
    success_detail: "You can now sign in from your Melodee app.",
};

const DE_DE: ResetCopy = ResetCopy {
    code: "de-DE",
    direction: "ltr",
    title: "Passwort zurücksetzen",
    instructions: "Wählen Sie ein neues Passwort für Ihr Melodee-Konto.",
    password: "Neues Passwort",
    confirm_password: "Neues Passwort bestätigen",
    submit: "Neues Passwort festlegen",
    checking: "Der Link wird überprüft…",
    missing: "Dieser Link enthält kein Token. Fordern Sie eine neue E-Mail zum Zurücksetzen des Passworts an.",
    invalid: "Dieser Link ist ungültig, abgelaufen oder wurde bereits verwendet. Fordern Sie eine neue E-Mail an.",
    too_short: "Das Passwort muss mindestens 8 Zeichen umfassen.",
    mismatch: "Die Passwörter stimmen nicht überein.",
    rate_limited: "Zu viele Versuche. Warten Sie bitte und versuchen Sie es später erneut.",
    unavailable: "Der Zurücksetzungsdienst ist nicht verfügbar. Versuchen Sie es später erneut.",
    success: "Ihr Passwort wurde zurückgesetzt.",
    success_detail: "Sie können sich jetzt in Ihrer Melodee-App anmelden.",
};

const ES_ES: ResetCopy = ResetCopy {
    code: "es-ES",
    direction: "ltr",
    title: "Restablece tu contraseña",
    instructions: "Elige una contraseña nueva para tu cuenta de Melodee.",
    password: "Nueva contraseña",
    confirm_password: "Confirma la nueva contraseña",
    submit: "Establecer nueva contraseña",
    checking: "Comprobando este enlace…",
    missing: "A este enlace le falta el token. Solicita un nuevo correo para restablecer la contraseña.",
    invalid: "Este enlace no es válido, ha caducado o ya se ha usado. Solicita un nuevo correo.",
    too_short: "La contraseña debe tener al menos 8 caracteres.",
    mismatch: "Las contraseñas no coinciden.",
    rate_limited: "Demasiados intentos. Espera e inténtalo de nuevo más tarde.",
    unavailable: "El servicio de restablecimiento no está disponible. Inténtalo más tarde.",
    success: "Tu contraseña se ha restablecido.",
    success_detail: "Ya puedes iniciar sesión desde tu aplicación Melodee.",
};

const FR_FR: ResetCopy = ResetCopy {
    code: "fr-FR",
    direction: "ltr",
    title: "Réinitialiser votre mot de passe",
    instructions: "Choisissez un nouveau mot de passe pour votre compte Melodee.",
    password: "Nouveau mot de passe",
    confirm_password: "Confirmer le nouveau mot de passe",
    submit: "Définir le nouveau mot de passe",
    checking: "Vérification de ce lien…",
    missing: "Ce lien ne contient pas de jeton. Demandez un nouvel e-mail de réinitialisation.",
    invalid: "Ce lien est invalide, expiré ou déjà utilisé. Demandez un nouvel e-mail.",
    too_short: "Le mot de passe doit comporter au moins 8 caractères.",
    mismatch: "Les mots de passe ne correspondent pas.",
    rate_limited: "Trop de tentatives. Attendez puis réessayez plus tard.",
    unavailable: "Le service de réinitialisation est indisponible. Réessayez plus tard.",
    success: "Votre mot de passe a été réinitialisé.",
    success_detail: "Vous pouvez maintenant vous connecter depuis votre application Melodee.",
};

const JA_JP: ResetCopy = ResetCopy {
    code: "ja-JP",
    direction: "ltr",
    title: "パスワードをリセット",
    instructions: "Melodee アカウントの新しいパスワードを設定してください。",
    password: "新しいパスワード",
    confirm_password: "新しいパスワード（確認）",
    submit: "新しいパスワードを設定",
    checking: "リセットリンクを確認しています…",
    missing: "このリセットリンクにはトークンがありません。新しいメールをリクエストしてください。",
    invalid: "このリンクは無効か期限切れ、または使用済みです。新しいメールをリクエストしてください。",
    too_short: "パスワードは8文字以上にしてください。",
    mismatch: "パスワードが一致しません。",
    rate_limited: "試行回数が多すぎます。しばらく待ってから再試行してください。",
    unavailable: "リセットサービスを利用できません。後でもう一度お試しください。",
    success: "パスワードをリセットしました。",
    success_detail: "Melodee アプリからサインインできます。",
};

const PT_BR: ResetCopy = ResetCopy {
    code: "pt-BR",
    direction: "ltr",
    title: "Redefina sua senha",
    instructions: "Escolha uma nova senha para sua conta Melodee.",
    password: "Nova senha",
    confirm_password: "Confirme a nova senha",
    submit: "Definir nova senha",
    checking: "Verificando este link…",
    missing: "Este link não contém um token. Solicite um novo e-mail de redefinição.",
    invalid: "Este link é inválido, expirou ou já foi usado. Solicite um novo e-mail.",
    too_short: "A senha deve ter pelo menos 8 caracteres.",
    mismatch: "As senhas não coincidem.",
    rate_limited: "Muitas tentativas. Aguarde e tente novamente mais tarde.",
    unavailable: "O serviço de redefinição está indisponível. Tente novamente mais tarde.",
    success: "Sua senha foi redefinida.",
    success_detail: "Agora você pode entrar pelo aplicativo Melodee.",
};

const AR_SA: ResetCopy = ResetCopy {
    code: "ar-SA",
    direction: "rtl",
    title: "إعادة تعيين كلمة المرور",
    instructions: "اختر كلمة مرور جديدة لحساب Melodee الخاص بك.",
    password: "كلمة المرور الجديدة",
    confirm_password: "تأكيد كلمة المرور الجديدة",
    submit: "تعيين كلمة مرور جديدة",
    checking: "جارٍ التحقق من رابط إعادة التعيين…",
    missing: "يفتقد هذا الرابط رمزًا. اطلب رسالة جديدة لإعادة تعيين كلمة المرور.",
    invalid: "هذا الرابط غير صالح أو منتهي الصلاحية أو استُخدم من قبل. اطلب رسالة جديدة.",
    too_short: "يجب أن تتكون كلمة المرور من 8 أحرف على الأقل.",
    mismatch: "كلمتا المرور غير متطابقتين.",
    rate_limited: "محاولات كثيرة جدًا. انتظر ثم حاول مرة أخرى لاحقًا.",
    unavailable: "خدمة إعادة التعيين غير متاحة. حاول مرة أخرى لاحقًا.",
    success: "تمت إعادة تعيين كلمة المرور.",
    success_detail: "يمكنك الآن تسجيل الدخول من تطبيق Melodee.",
};

const FA_IR: ResetCopy = ResetCopy {
    code: "fa-IR",
    direction: "rtl",
    title: "بازنشانی رمز عبور",
    instructions: "برای حساب Melodee خود یک رمز عبور جدید انتخاب کنید.",
    password: "رمز عبور جدید",
    confirm_password: "تأیید رمز عبور جدید",
    submit: "تنظیم رمز عبور جدید",
    checking: "در حال بررسی این پیوند…",
    missing: "این پیوند توکن ندارد. یک ایمیل جدید برای بازنشانی رمز عبور درخواست کنید.",
    invalid: "این پیوند نامعتبر، منقضی یا قبلاً استفاده شده است. یک ایمیل جدید درخواست کنید.",
    too_short: "رمز عبور باید دست‌کم ۸ نویسه داشته باشد.",
    mismatch: "رمزهای عبور یکسان نیستند.",
    rate_limited: "تلاش‌های زیادی انجام شده است. بعداً دوباره امتحان کنید.",
    unavailable: "سرویس بازنشانی در دسترس نیست. بعداً دوباره امتحان کنید.",
    success: "رمز عبور شما بازنشانی شد.",
    success_detail: "اکنون می‌توانید از برنامه Melodee وارد شوید.",
};

// These cultures intentionally use the en-US source copy until a dedicated
// browser-flow translation is available. Keeping the culture code and RTL
// direction intact follows ADR-0137's established English-fallback semantics
// instead of silently selecting an unrelated language.
const IT_IT: ResetCopy = ResetCopy {
    code: "it-IT",
    ..EN_US
};
const RU_RU: ResetCopy = ResetCopy {
    code: "ru-RU",
    ..EN_US
};
const ZH_CN: ResetCopy = ResetCopy {
    code: "zh-CN",
    ..EN_US
};
const NL_NL: ResetCopy = ResetCopy {
    code: "nl-NL",
    ..EN_US
};
const PL_PL: ResetCopy = ResetCopy {
    code: "pl-PL",
    ..EN_US
};
const TR_TR: ResetCopy = ResetCopy {
    code: "tr-TR",
    ..EN_US
};
const ID_ID: ResetCopy = ResetCopy {
    code: "id-ID",
    ..EN_US
};
const KO_KR: ResetCopy = ResetCopy {
    code: "ko-KR",
    ..EN_US
};
const VI_VN: ResetCopy = ResetCopy {
    code: "vi-VN",
    ..EN_US
};
const UK_UA: ResetCopy = ResetCopy {
    code: "uk-UA",
    ..EN_US
};
const CS_CZ: ResetCopy = ResetCopy {
    code: "cs-CZ",
    ..EN_US
};
const SV_SE: ResetCopy = ResetCopy {
    code: "sv-SE",
    ..EN_US
};

fn language_copy(value: Option<&HeaderValue>) -> &'static ResetCopy {
    let Some(value) = value.and_then(|value| value.to_str().ok()) else {
        return &EN_US;
    };
    for language in value.split(',') {
        let language = language
            .split(';')
            .next()
            .map(str::trim)
            .unwrap_or_default()
            .to_ascii_lowercase();
        let copy = match language.as_str() {
            "de-de" | "de" => &DE_DE,
            "es-es" | "es" => &ES_ES,
            "fr-fr" | "fr" => &FR_FR,
            "it-it" | "it" => &IT_IT,
            "ja-jp" | "ja" => &JA_JP,
            "pt-br" | "pt" => &PT_BR,
            "ru-ru" | "ru" => &RU_RU,
            "zh-cn" | "zh" => &ZH_CN,
            "ar-sa" | "ar" => &AR_SA,
            "nl-nl" | "nl" => &NL_NL,
            "pl-pl" | "pl" => &PL_PL,
            "tr-tr" | "tr" => &TR_TR,
            "id-id" | "id" => &ID_ID,
            "ko-kr" | "ko" => &KO_KR,
            "vi-vn" | "vi" => &VI_VN,
            "fa-ir" | "fa" => &FA_IR,
            "uk-ua" | "uk" => &UK_UA,
            "cs-cz" | "cs" => &CS_CZ,
            "sv-se" | "sv" => &SV_SE,
            "en-us" | "en" => &EN_US,
            _ => continue,
        };
        return copy;
    }
    &EN_US
}

fn js_string(value: &str) -> String {
    match serde_json::to_string(value) {
        Ok(value) => value,
        Err(_) => "\"\"".to_owned(),
    }
}

fn render_page(copy: &ResetCopy, nonce: &str) -> String {
    let template = r##"<!doctype html>
<html lang="__LANG__" dir="__DIR__">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>__TITLE__ · Melodee</title>
  <style>
    :root { color-scheme: light dark; font-family: system-ui, sans-serif; background: #f7f7f7; color: #161616; }
    @media (prefers-color-scheme: dark) { :root { background: #161616; color: #f4f4f4; } }
    body { margin: 0; min-block-size: 100vh; display: grid; place-items: center; padding: 1rem; box-sizing: border-box; }
    main { inline-size: min(100%, 32rem); padding: 2rem; border: 1px solid #8d8d8d; border-radius: .5rem; background: Canvas; box-sizing: border-box; }
    h1 { margin-block-start: 0; font-size: 1.75rem; }
    p { line-height: 1.5; }
    label { display: block; margin-block: 1rem .35rem; font-weight: 600; }
    input { box-sizing: border-box; inline-size: 100%; padding: .7rem; border: 1px solid #525252; border-radius: .25rem; font: inherit; background: Canvas; color: CanvasText; }
    button { margin-block-start: 1.25rem; padding: .75rem 1rem; border: 0; border-radius: .25rem; font: inherit; font-weight: 700; background: #0f62fe; color: white; cursor: pointer; }
    button:disabled { cursor: wait; opacity: .65; }
    [role="status"] { margin-block-start: 1rem; }
    .success { color: #198038; }
    .error { color: #da1e28; }
    [hidden] { display: none !important; }
  </style>
</head>
<body>
  <main>
    <h1>__TITLE__</h1>
    <p id="instructions">__INSTRUCTIONS__</p>
    <p id="status" role="status" aria-live="polite">__CHECKING__</p>
    <form id="reset-form" hidden novalidate>
      <label for="password">__PASSWORD__</label>
      <input id="password" name="password" type="password" autocomplete="new-password" minlength="8" required>
      <label for="confirm-password">__CONFIRM_PASSWORD__</label>
      <input id="confirm-password" name="confirm-password" type="password" autocomplete="new-password" minlength="8" required>
      <button id="submit" type="submit">__SUBMIT__</button>
    </form>
    <section id="success" class="success" hidden aria-live="polite">
      <p>__SUCCESS__</p>
      <p>__SUCCESS_DETAIL__</p>
    </section>
  </main>
  <script nonce="__NONCE__">
    (() => {
      'use strict';
      const copy = {
        checking: __CHECKING_JS__,
        missing: __MISSING_JS__,
        invalid: __INVALID_JS__,
        tooShort: __TOO_SHORT_JS__,
        mismatch: __MISMATCH_JS__,
        rateLimited: __RATE_LIMITED_JS__,
        unavailable: __UNAVAILABLE_JS__,
      };
      const status = document.getElementById('status');
      const form = document.getElementById('reset-form');
      const password = document.getElementById('password');
      const confirmation = document.getElementById('confirm-password');
      const submit = document.getElementById('submit');
      const success = document.getElementById('success');
      let resetToken = new URL(window.location.href).searchParams.get('token');

      // The query string is a credential. Remove it before validation starts,
      // so normal navigation and subsequent same-origin requests cannot carry
      // it in a Referer header. No storage API or third-party request is used.
      window.history.replaceState(null, document.title, window.location.pathname);

      const showError = (message) => {
        status.textContent = message;
        status.className = 'error';
      };

      if (!resetToken) {
        showError(copy.missing);
        return;
      }
      if (resetToken.length > 512 || !/^[A-Za-z0-9_-]+$/.test(resetToken)) {
        showError(copy.invalid);
        return;
      }

      const validateUrl = '/api/v1/auth/password-reset/validate/' + encodeURIComponent(resetToken);
      fetch(validateUrl, {
        method: 'GET',
        headers: { Accept: 'application/json' },
        credentials: 'omit',
        cache: 'no-store',
        redirect: 'error',
      }).then((response) => {
        if (!response.ok) {
          throw new Error('invalid');
        }
        return response.json();
      }).then((body) => {
        if (body.valid !== true) {
          throw new Error('invalid');
        }
        status.textContent = '';
        status.className = '';
        form.hidden = false;
        password.focus();
      }).catch((error) => {
        // Network failures are intentionally collapsed to a stable message;
        // the server's response body and the token never reach the page.
        if (error instanceof TypeError) {
          showError(copy.unavailable);
        } else {
          showError(copy.invalid);
        }
      });

      form.addEventListener('submit', (event) => {
        event.preventDefault();
        if (password.value.length < 8) {
          showError(copy.tooShort);
          password.focus();
          return;
        }
        if (password.value !== confirmation.value) {
          showError(copy.mismatch);
          confirmation.focus();
          return;
        }
        submit.disabled = true;
        fetch('/api/v1/auth/password-reset/confirm', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
          credentials: 'omit',
          cache: 'no-store',
          redirect: 'error',
          body: JSON.stringify({ token: resetToken, newPassword: password.value }),
        }).then((response) => {
          if (response.status === 429) {
            throw new Error('rate-limited');
          }
          if (!response.ok) {
            throw new Error('invalid');
          }
          return response.json();
        }).then(() => {
          resetToken = null;
          password.value = '';
          confirmation.value = '';
          form.hidden = true;
          status.hidden = true;
          success.hidden = false;
        }).catch((error) => {
          submit.disabled = false;
          if (error instanceof TypeError) {
            showError(copy.unavailable);
          } else if (error.message === 'rate-limited') {
            showError(copy.rateLimited);
          } else {
            showError(copy.invalid);
          }
        });
      });
    })();
  </script>
</body>
</html>
"##;
    template
        .replace("__LANG__", copy.code)
        .replace("__DIR__", copy.direction)
        .replace("__TITLE__", copy.title)
        .replace("__INSTRUCTIONS__", copy.instructions)
        .replace("__PASSWORD__", copy.password)
        .replace("__CONFIRM_PASSWORD__", copy.confirm_password)
        .replace("__SUBMIT__", copy.submit)
        .replace("__CHECKING__", copy.checking)
        .replace("__MISSING_JS__", &js_string(copy.missing))
        .replace("__INVALID_JS__", &js_string(copy.invalid))
        .replace("__TOO_SHORT_JS__", &js_string(copy.too_short))
        .replace("__MISMATCH_JS__", &js_string(copy.mismatch))
        .replace("__RATE_LIMITED_JS__", &js_string(copy.rate_limited))
        .replace("__UNAVAILABLE_JS__", &js_string(copy.unavailable))
        .replace("__CHECKING_JS__", &js_string(copy.checking))
        .replace("__SUCCESS__", copy.success)
        .replace("__SUCCESS_DETAIL__", copy.success_detail)
        .replace("__NONCE__", nonce)
}

fn no_store_headers(response: &mut Response, nonce: &str) {
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, private"),
    );
    headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    headers.insert(
        header::EXPIRES,
        HeaderValue::from_static("Thu, 01 Jan 1970 00:00:00 GMT"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    let csp = format!(
        "default-src 'none'; script-src 'nonce-{nonce}'; style-src 'unsafe-inline'; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'"
    );
    let csp = match HeaderValue::from_str(&csp) {
        Ok(value) => value,
        Err(_) => HeaderValue::from_static("default-src 'none'; object-src 'none'"),
    };
    headers.insert(header::CONTENT_SECURITY_POLICY, csp);
    headers.insert(header::VARY, HeaderValue::from_static("Accept-Language"));
    headers.insert(
        HeaderName::from_static("cross-origin-resource-policy"),
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        HeaderName::from_static("cross-origin-opener-policy"),
        HeaderValue::from_static("same-origin"),
    );
}

async fn password_reset_page(headers: HeaderMap) -> Response {
    let copy = language_copy(headers.get(header::ACCEPT_LANGUAGE));
    let nonce = Uuid::now_v7().to_string();
    let mut response = (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        render_page(copy, &nonce),
    )
        .into_response();
    no_store_headers(&mut response, &nonce);
    response
}

/// Builds the anonymous, bounded browser password-reset endpoint.
pub fn router() -> Router {
    Router::new().route("/reset-password", get(password_reset_page))
}

#[cfg(test)]
mod tests {
    use super::{AR_SA, DE_DE, EN_US, language_copy, render_page};
    use axum::http::HeaderValue;

    #[test]
    fn language_selection_uses_project_cultures_and_english_fallback() {
        assert_eq!(language_copy(None).code, EN_US.code);
        assert_eq!(
            language_copy(Some(&HeaderValue::from_static("de-CH,de;q=0.8"))).code,
            DE_DE.code
        );
        assert_eq!(
            language_copy(Some(&HeaderValue::from_static("ar-SA"))).direction,
            AR_SA.direction
        );
        assert_eq!(
            language_copy(Some(&HeaderValue::from_static("xx"))).code,
            EN_US.code
        );
    }

    #[test]
    fn rendered_page_contains_no_external_resource_or_token_storage_path() {
        let page = render_page(&EN_US, "test-nonce");
        assert!(page.contains("credentials: 'omit'"));
        assert!(page.contains("cache: 'no-store'"));
        assert!(page.contains("history.replaceState"));
        assert!(!page.contains("localStorage"));
        assert!(!page.contains("sessionStorage"));
        assert!(!page.contains("<img"));
        assert!(!page.contains("<link"));
        assert!(!page.contains("http://"));
        assert!(!page.contains("https://"));
    }
}
