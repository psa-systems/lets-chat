# LC-188: Auth flow chrome (login extras, register, forgot, reset, email
# verification). Message ids are kebab-case, area-prefixed; Fluent ids cannot
# contain ".". Keep this file in sync with es/auth.ftl (CI checks coverage).
# Core login keys (login-title/username/password/submit/register-link) live in
# main.ftl and are reused here.

## Login (extras)
login-page-title = Sign in
login-forgot-link = Forgot password?
login-no-account = No account?
login-logo-alt = Logo
login-bunyip-submit = Sign in with Bunyip
login-bunyip-redirect-notice = You will be redirected to { $host } to sign in.

## Sign-in approval (bunyip/approve)
login-approve-title = Approve sign-in
login-approve-heading = Approve this sign-in
login-approve-intro = This sign-in looked unfamiliar, so we emailed a 6-digit code to the address on your account. Enter it below to finish signing in.
login-approve-code-label = 6-digit approval code
login-approve-submit = Approve sign-in
login-approve-expiry = The code expires in 15 minutes. If this was not you, close this page - the sign-in cannot complete without the code.

## Register
register-page-title = Register
register-title = Register
register-username = Username
register-email = Email
register-email-optional = (optional)
register-email-help = Used only for password reset. Set later in Settings if you skip it now.
register-password = Password
register-confirm-password = Confirm password
register-honeypot-label = Leave this field empty
register-submit = Create account
register-have-account = Have an account?
register-signin-link = Sign in

## Forgot password
forgot-page-title = Forgot password
forgot-title = Forgot password
forgot-help = Enter your account email. If it matches a registered user, we'll send a link that lets you set a new password.
forgot-email = Email
forgot-submit = Send reset link
forgot-remembered = Remembered it?
forgot-signin-link = Sign in

## Reset password
reset-page-title = Reset password
reset-title = Choose a new password
reset-new-password = New password
reset-confirm-password = Confirm new password
reset-submit = Save new password

## Email verification result
verify-email-page-title = Email verification
verify-email-title = Email verification
verify-email-open-link = Open Let's Chat
verify-email-signin-link = Sign in
