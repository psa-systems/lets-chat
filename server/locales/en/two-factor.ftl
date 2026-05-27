# LC-188: Two-factor authentication chrome (login challenge, recovery,
# enrollment setup, recovery-code confirmation, registration setup). Message ids
# are kebab-case, area-prefixed; Fluent ids cannot contain ".". Keep in sync
# with es/two-factor.ftl (CI checks coverage).

## Shared (enrollment QR + secret + code input)
twofactor-qr-alt = TOTP QR Code
twofactor-cant-scan = Can't scan? Use this secret instead
twofactor-secret-account = Account
twofactor-secret-issuer = Issuer
twofactor-secret-algorithm = Algorithm
twofactor-secret-digits = digits
twofactor-code-label = 6-digit code

## Login challenge
twofactor-login-page-title = Two-factor code
twofactor-login-title = Two-factor code
twofactor-login-help = Enter the 6-digit code from your authenticator app.
twofactor-login-code = Code
twofactor-login-verify = Verify
twofactor-login-lost-device = Lost your device?
twofactor-login-recovery-link = Use a recovery code

## Recovery code challenge
twofactor-recovery-page-title = Recovery code
twofactor-recovery-title = Recovery code
twofactor-recovery-help = Enter one of the recovery codes you saved when you set up two-factor authentication.
twofactor-recovery-code-label = Recovery code
twofactor-recovery-verify = Verify
twofactor-recovery-back-link = Back to authenticator code

## Enrollment setup
twofactor-setup-page-title = Set up two-factor authentication
twofactor-setup-title = Set up two-factor authentication
twofactor-setup-help = Two-factor authentication is required for every account. Scan the QR code below with an authenticator app (1Password, Authy, Google Authenticator) and enter the 6-digit code to finish enrollment.
twofactor-setup-submit = Verify and enable
twofactor-setup-signout = Sign out

## Recovery-code confirmation
twofactor-confirm-page-title = Save your recovery codes
twofactor-confirm-title = Save your recovery codes
twofactor-confirm-help = Each code works once. Store them somewhere safe. If you lose your authenticator, a recovery code is the only way back into your account.
twofactor-confirm-warning = You will not see these again. Copy them now.
twofactor-confirm-continue = I saved my codes - continue

## Registration setup
twofactor-register-page-title = Finish registration
twofactor-register-title = Finish registration
twofactor-register-help = Two-factor authentication is required on this server. Your account is not created yet; scan the QR code with an authenticator app (1Password, Authy, Google Authenticator) and enter the 6-digit code to finalize registration.
twofactor-register-submit = Verify and create account
twofactor-register-cancel = Cancel and start over
