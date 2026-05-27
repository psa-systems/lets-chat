# LC-188: Catálogo en español de la autenticación de dos factores. Debe definir
# los mismos ids que en/two-factor.ftl (CI verifica la cobertura). Las claves
# Fluent no pueden contener ".".

## Compartido (QR de inscripción + secreto + entrada de código)
twofactor-qr-alt = Código QR TOTP
twofactor-cant-scan = ¿No puedes escanear? Usa este secreto en su lugar
twofactor-secret-account = Cuenta
twofactor-secret-issuer = Emisor
twofactor-secret-algorithm = Algoritmo
twofactor-secret-digits = dígitos
twofactor-code-label = Código de 6 dígitos

## Desafío de inicio de sesión
twofactor-login-page-title = Código de dos factores
twofactor-login-title = Código de dos factores
twofactor-login-help = Introduce el código de 6 dígitos de tu aplicación de autenticación.
twofactor-login-code = Código
twofactor-login-verify = Verificar
twofactor-login-lost-device = ¿Perdiste tu dispositivo?
twofactor-login-recovery-link = Usar un código de recuperación

## Desafío con código de recuperación
twofactor-recovery-page-title = Código de recuperación
twofactor-recovery-title = Código de recuperación
twofactor-recovery-help = Introduce uno de los códigos de recuperación que guardaste cuando configuraste la autenticación de dos factores.
twofactor-recovery-code-label = Código de recuperación
twofactor-recovery-verify = Verificar
twofactor-recovery-back-link = Volver al código de autenticación

## Configuración de inscripción
twofactor-setup-page-title = Configurar autenticación de dos factores
twofactor-setup-title = Configurar autenticación de dos factores
twofactor-setup-help = La autenticación de dos factores es obligatoria para todas las cuentas. Escanea el código QR de abajo con una aplicación de autenticación (1Password, Authy, Google Authenticator) e introduce el código de 6 dígitos para completar la inscripción.
twofactor-setup-submit = Verificar y activar
twofactor-setup-signout = Cerrar sesión

## Confirmación de códigos de recuperación
twofactor-confirm-page-title = Guarda tus códigos de recuperación
twofactor-confirm-title = Guarda tus códigos de recuperación
twofactor-confirm-help = Cada código funciona una sola vez. Guárdalos en un lugar seguro. Si pierdes tu autenticador, un código de recuperación es la única forma de volver a entrar en tu cuenta.
twofactor-confirm-warning = No volverás a verlos. Cópialos ahora.
twofactor-confirm-continue = Guardé mis códigos - continuar

## Configuración de registro
twofactor-register-page-title = Finalizar el registro
twofactor-register-title = Finalizar el registro
twofactor-register-help = La autenticación de dos factores es obligatoria en este servidor. Tu cuenta aún no se ha creado; escanea el código QR con una aplicación de autenticación (1Password, Authy, Google Authenticator) e introduce el código de 6 dígitos para finalizar el registro.
twofactor-register-submit = Verificar y crear cuenta
twofactor-register-cancel = Cancelar y empezar de nuevo
