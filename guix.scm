;;; Build guile-lsp with Guix:   guix build -f guix.scm
;;; or enter a shell with it:    guix shell -f guix.scm
;;; The crate sources come from packaging/guile-lsp-crates.scm.

(use-modules (guix packages)
             (guix gexp)
             (guix build-system cargo)
             (guix licenses)
             (gnu packages guile)
             (gnu packages bash))

(add-to-load-path (dirname (current-filename)))
(use-modules (packaging guile-lsp-crates))

(define-public guile-lsp
  (package
    (name "guile-lsp")
    (version "0.1.0")
    (source (local-file (dirname (current-filename)) "guile-lsp-checkout"
                        #:recursive? #t
                        #:select? (lambda (file stat)
                                    (not (member (basename file)
                                                 '("target" ".git" ".direnv"))))))
    (build-system cargo-build-system)
    (arguments
     (list #:install-source? #f
           ;; Integration tests start Guile and Guix processes; the unit
           ;; tests cover the reader, formatter, and analysis.
           #:cargo-test-flags ''("--lib")
           #:phases
           #~(modify-phases %standard-phases
               (add-after 'install 'wrap
                 (lambda* (#:key inputs #:allow-other-keys)
                   ;; The Guile worker sources are embedded in the binary;
                   ;; a Guile must be on PATH unless guile.command says
                   ;; otherwise (`guix repl --` for the Guix features).
                   (wrap-program (string-append #$output "/bin/guile-lsp")
                     `("PATH" ":" suffix
                       (,(dirname (search-input-file inputs "bin/guile"))))))))))
    (inputs (cons* bash-minimal guile-3.0
                   (cargo-inputs 'guile-lsp #:module '(packaging guile-lsp-crates))))
    (home-page "https://example.invalid/guile-lsp")
    (synopsis "Language server for GNU Guile with optional Guix integration")
    (description
     "guile-lsp is a language server for Guile: completion, hover, definitions,
diagnostics, and formatting from a tolerant reader and a supervised Guile
worker.  With @command{guix repl} as the worker it also understands Guix
records, service types, and configurations, and can validate a configuration
without building it.")
    (license gpl3+)))

guile-lsp
