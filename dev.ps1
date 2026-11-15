# Zažene MailHub v razvojnem načinu.
# Na tem racunalniku je v PATH najprej nameščen "Rust stable GNU", ki ne zna
# povezati programa, zato uporabimo rustup orodja (MSVC).
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
Set-Location $PSScriptRoot
npm run tauri dev
