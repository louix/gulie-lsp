;;; guile-lsp worker: a long-lived process the Rust server talks to over stdio.
;;;
;;; Protocol version 1. Both directions use LSP-style framing:
;;;   Content-Length: N\r\n\r\n<N bytes of UTF-8 holding one datum>
;;;
;;; Server -> worker:
;;;   (request ID METHOD PARAMS)   METHOD is a symbol, PARAMS an alist
;;;   (cancel ID)                  best effort; SIGUSR1 interrupts a running request
;;;   (shutdown)
;;; Worker -> server:
;;;   (response ID ok RESULT)
;;;   (response ID error KIND "message")
;;;       KIND: cancelled | restricted | not-found | bad-request | unknown-method | failed
;;;   (log LEVEL "message")        LEVEL: info | warning
;;;
;;; stdout carries protocol frames only; everything else goes to stderr.
;;; Requests are handled one at a time, in order.
;;;
;;;   guile -L <dir> <dir>/guile-lsp/worker.scm --trust restricted|trusted

(define-module (guile-lsp worker)
  #:use-module (guile-lsp metadata)
  #:use-module (ice-9 match)
  #:use-module (ice-9 binary-ports)
  #:use-module (ice-9 rdelim)
  #:use-module (ice-9 ftw)
  #:use-module (ice-9 textual-ports)
  #:use-module (rnrs bytevectors)
  #:use-module (srfi srfi-1)
  #:export (main))

(define protocol-version 1)
(define worker-version "0.1.0")

(define trust 'restricted)
(define protocol-out (current-output-port))


;;;
;;; Framing.
;;;

(define (write-frame datum)
  (let ((bytes (string->utf8 (call-with-output-string (lambda (port) (write datum port))))))
    (put-bytevector protocol-out
                    (string->utf8 (format #f "Content-Length: ~a\r\n\r\n" (bytevector-length bytes))))
    (put-bytevector protocol-out bytes)
    (force-output protocol-out)))

(define (read-frame port)
  "Read one framed datum, or the EOF object when the server closed the pipe."
  (let loop ((length #f))
    (let ((line (read-line port)))
      (cond ((eof-object? line) line)
            ((string=? (string-trim-right line #\return) "")
             (if length
                 (let ((bytes (get-bytevector-n port length)))
                   (if (eof-object? bytes)
                       bytes
                       (call-with-input-string (utf8->string bytes) read)))
                 (loop #f)))
            ((string-prefix-ci? "content-length:" line)
             (loop (string->number (string-trim-both (substring line 15) (lambda (c) (or (char-whitespace? c) (eqv? c #\return)))))))
            (else (loop length))))))

(define (log-message level message)
  (write-frame `(log ,level ,message)))


;;;
;;; Cancellation: SIGUSR1 aborts the request in progress, if any.
;;;

(define cancel-tag (make-prompt-tag "guile-lsp-cancel"))
(define request-running? #f)

(define (install-cancellation!)
  (sigaction SIGUSR1
    (lambda (signal)
      (when request-running?
        (catch #t
          (lambda () (abort-to-prompt cancel-tag))
          (lambda _ #f))))))


;;;
;;; Environment identity.
;;;

(define (guix-identity)
  "Describe the running Guix, or #f when (guix describe) is unavailable."
  (let ((describe (resolve-module '(guix describe) #:ensure #f)))
    (and describe
         (catch #t
           (lambda ()
             (let* ((channels ((module-ref describe 'current-channels)))
                    (channel-module (resolve-module '(guix channels)))
                    ;; Record accessors are macros: apply them by evaluation.
                    (name (eval '(lambda (c) (channel-name c)) channel-module))
                    (commit (eval '(lambda (c) (channel-commit c)) channel-module)))
               `((profile . ,((module-ref describe 'current-profile)))
                 (channels . ,(map (lambda (c) (list (name c) (commit c))) channels)))))
           (lambda (key . args)
             `((unavailable . ,(object->string (cons key args)))))))))

(define (hello params)
  `((protocol . ,protocol-version)
    (worker . ,worker-version)
    (guile . ,(version))
    (effective-version . ,(effective-version))
    (pid . ,(getpid))
    (trust . ,trust)
    (load-path . ,%load-path)
    (guix . ,(guix-identity))))


;;;
;;; Methods.
;;;

(define (param params key)
  (assq-ref params key))

(define (require-trust!)
  (unless (eq? trust 'trusted)
    (throw 'guile-lsp-error 'restricted
           "loading modules requires workspace trust; only source-only requests are available")))

(define (module-name-param params key)
  (let ((name (param params key)))
    (unless (and (list? name) (pair? name) (every symbol? name))
      (throw 'guile-lsp-error 'bad-request (format #f "expected a module name for ~a" key)))
    name))

(define (lookup params)
  "Find NAME in the first of MODULES that binds it. Modules that fail to load
are skipped and reported in the result."
  (require-trust!)
  (let ((name (param params 'name))
        (modules (param params 'modules)))
    (unless (symbol? name) (throw 'guile-lsp-error 'bad-request "expected a symbol name"))
    (let loop ((modules modules) (failures '()))
      (if (null? modules)
          (throw 'guile-lsp-error 'not-found
                 (format #f "~a is not bound in ~a" name
                         (string-join (map object->string (param params 'modules)) ", ")))
          (let* ((module-name (car modules))
                 (module (catch #t
                           (lambda () (resolve-interface module-name))
                           (lambda (key . args) #f)))
                 (variable (and module (module-variable module name))))
            (cond ((and variable (variable-bound? variable))
                   (let ((info (binding-info name (variable-ref variable)
                                             #:module module #:variable variable)))
                     `((module . ,module-name)
                       (binding . ,info)
                       (failed-modules . ,(reverse failures)))))
                  (else (loop (cdr modules)
                              (if module failures (cons module-name failures))))))))))

(define (exports params)
  (require-trust!)
  (module-exports (module-name-param params 'module)
                  #:private? (eq? (param params 'private?) #t)))

(define (source-records* params)
  (let ((file (param params 'file)))
    (unless (string? file) (throw 'guile-lsp-error 'bad-request "expected a file path"))
    (source-records file)))

(define (configuration params)
  (require-trust!)
  (guix-configuration-fields (module-name-param params 'module) (param params 'name)))

(define (service-type params)
  (require-trust!)
  (guix-service-type-summary (module-name-param params 'module) (param params 'name)))

(define (add-load-path params)
  "Prepend PATH to the load path, as -L would. Not trust-gated: nothing loads."
  (let ((path (param params 'path)))
    (unless (string? path) (throw 'guile-lsp-error 'bad-request "expected a directory path"))
    (unless (member path %load-path)
      (set! %load-path (cons path %load-path)))
    `((load-path . ,%load-path))))

(define (scan-modules)
  "Module names found as .scm files under the load path, deduplicated.
Directories that do not exist are skipped; nothing is loaded."
  (define (scm-files dir)
    (let walk ((dir dir) (prefix '()) (acc '()))
      (let ((entries (catch #t (lambda () (scandir dir)) (lambda _ #f))))
        (if (not entries)
            acc
            (fold (lambda (entry acc)
                    (cond ((member entry '("." "..")) acc)
                          ((string-suffix? ".scm" entry)
                           (cons (reverse (cons (string->symbol (substring entry 0 (- (string-length entry) 4))) prefix)) acc))
                          ((and (not (string-prefix? "." entry))
                                (file-is-directory? (string-append dir "/" entry)))
                           (walk (string-append dir "/" entry) (cons (string->symbol entry) prefix) acc))
                          (else acc)))
                  acc entries)))))
  (let ((seen (make-hash-table)))
    (filter (lambda (name) (and (not (hash-ref seen name)) (hash-set! seen name #t)))
            (append-map scm-files %load-path))))

(define module-cache #f)          ; (load-path . names)

(define (list-modules params)
  "All module names on the load path. Cached until the load path changes."
  (unless (and module-cache (equal? (car module-cache) %load-path))
    (set! module-cache (cons %load-path (scan-modules))))
  `((modules . ,(cdr module-cache))))


(define (find-exporters params)
  "Loaded modules whose public interface exports NAME, Guix and gnu modules
first, then Guile's own.  Only modules already loaded are searched, so
nothing runs; under Guix that covers whatever the metadata requests have
brought in, which is most of what a configuration imports."
  (let* ((name (string->symbol (assq-ref params 'name)))
         (found '()))
    (define (walk module)
      (let ((iface (module-public-interface module)))
        (when (and iface (module-variable iface name)
                   (not (member (module-name module) found)))
          (set! found (cons (module-name module) found))))
      (hash-for-each (lambda (_ child) (walk child)) (module-submodules module)))
    (walk (resolve-module '() #f))
    (define (rank module-name)
      (match module-name
        (('gnu . _) 0)
        (('guix . _) 1)
        (('srfi . _) 2)
        (('ice-9 . _) 3)
        (_ 4)))
    `((modules . ,(sort found (lambda (a b)
                               (or (< (rank a) (rank b))
                                   (and (= (rank a) (rank b))
                                        (string<? (format #f "~a" a)
                                                  (format #f "~a" b))))))))))

(define (module-file params)
  "The source file of the module named in PARAMS, found on the load path
without loading it, or a not-found error."
  (let* ((name (assq-ref params 'module))
         (relative (string-append (string-join (map symbol->string name) "/") ".scm"))
         (file (%search-load-path relative)))
    (unless file
      (throw 'guile-lsp-error 'not-found
             (format #f "no file for module ~a on the load path" name)))
    `((file . ,file))))
(define methods
  `((hello . ,hello)
    (add-load-path . ,add-load-path)
    (list-modules . ,list-modules)
    (find-exporters . ,find-exporters)
    (module-file . ,module-file)
    (lookup . ,lookup)
    (exports . ,exports)
    (source-records . ,source-records*)
    (configuration . ,configuration)
    (service-type . ,service-type)))


;;;
;;; Dispatch.
;;;

(define (describe-error key args)
  "Render a caught exception the way Guile's REPL would, without a backtrace."
  (match args
    ((subr (? string? message) (? list? format-args) . _)
     (string-append (if subr (format #f "~a: " subr) "")
                    (catch #t
                      (lambda () (apply format #f message format-args))
                      (lambda _ message))))
    (_ (format #f "~a ~s" key args))))

(define (run-request id method params)
  (let ((handler (assq-ref methods method)))
    (if (not handler)
        `(response ,id error unknown-method ,(format #f "unknown method ~a" method))
        (call-with-prompt cancel-tag
          (lambda ()
            (set! request-running? #t)
            (let ((result
                   (catch #t
                     (lambda () `(response ,id ok ,(handler params)))
                     (lambda (key . args)
                       (match (cons key args)
                         (('guile-lsp-error kind message)
                          `(response ,id error ,kind ,message))
                         (_ `(response ,id error failed ,(describe-error key args))))))))
              (set! request-running? #f)
              result))
          (lambda (continuation . _)
            (set! request-running? #f)
            `(response ,id error cancelled "request cancelled"))))))

(define (serve port)
  (let loop ()
    (let ((message (read-frame port)))
      (match message
        ((? eof-object?) #t)
        (('shutdown) #t)
        (('cancel id) (loop))               ; nothing queued here: requests are sequential
        (('request id (? symbol? method) params)
         (write-frame (run-request id method (if (list? params) params '())))
         (loop))
        (other
         (log-message 'warning (format #f "ignoring malformed message ~s" other))
         (loop))))))

(define (main args)
  (let loop ((args (cdr args)))
    (match args
      (("--trust" level . rest)
       (set! trust (string->symbol level))
       (loop rest))
      ((unknown . rest)
       (format (current-error-port) "guile-lsp worker: ignoring argument ~a~%" unknown)
       (loop rest))
      (() #t)))
  ;; Protocol frames must never be interleaved with stray output.
  (set-current-output-port (current-error-port))
  (setvbuf protocol-out 'block)
  (install-cancellation!)
  (let ((in (current-input-port)))
    (setvbuf in 'block)
    (serve in)))
