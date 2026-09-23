;;; Validate a Guix configuration file the way `guix system` and `guix home`
;;; read it, without building anything, and report the outcome as one
;;; S-expression on stdout:
;;;
;;;   (validation (kind operating-system) (summary "host fixture, 42 services")
;;;               (checks "...") (errors ((message "...") (file "...")
;;;                                       (line 12) (column 4) (hint "..."))))
;;;
;;; What is checked: the file loads in a fresh user module with the modules
;;; `guix system`/`guix home` provide; the value is an operating system, home
;;; environment, package, or manifest; and for systems and home environments
;;; the service graph folds (every extended service type is present, no
;;; duplicates, extension procedures run). Derivations are not computed, so
;;; nothing is added to the store and no daemon is contacted. Errors from
;;; that later stage (missing packages' sources, gexp lowering) are not seen.

(define-module (guile-lsp validate)
  #:use-module (ice-9 match)
  #:use-module (srfi srfi-1)
  #:use-module (srfi srfi-34)
  #:use-module (srfi srfi-35)
  #:declarative? #f
  #:export (validate-file main))

(define system-modules
  '((gnu system) (gnu services) (gnu system shadow)))

(define home-modules
  '((gnu home)))

(define (make-user-module modules)
  (let ((module (make-fresh-user-module)))
    (for-each (lambda (iface)
                (module-use! module (resolve-interface iface)))
              modules)
    module))

(define (location-alist loc)
  ;; Guix <location>: 1-based line, 0-based column. Accessors are macros in
  ;; (guix diagnostics), so go through eval.
  (let ((diagnostics (resolve-interface '(guix diagnostics))))
    (define (field name)
      ((eval `(lambda (l) (,name l)) (resolve-module '(guix diagnostics))) loc))
    `((file . ,(field 'location-file))
      (line . ,(field 'location-line))
      (column . ,(field 'location-column)))))

(define (frame-location frame)
  (match (and frame (frame-source frame))
    (((? number?) (? string? file) (? number? line) . (? number? column)) ; Guile 3: (addr file line . column), 0-based line
     `((file . ,file) (line . ,(+ line 1)) (column . ,column)))
    (#(file line column)               ; 0-based line
     (and file `((file . ,file) (line . ,(+ line 1)) (column . ,column))))
    ((('line . line) ('column . column) ('filename . file))
     (and file `((file . ,file) (line . ,(+ line 1)) (column . ,column))))
    (_ #f)))

(define (last-frame-with-source stack file)
  "The innermost frame whose source is FILE, else the innermost with any
source outside boot-9."
  (let loop ((i 0) (fallback #f))
    (if (>= i (stack-length stack))
        fallback
        (let* ((frame (stack-ref stack i))
               (loc (frame-location frame)))
          (cond ((and loc (string=? (assq-ref loc 'file) file)) frame)
                ((and loc (not fallback)
                      (not (eq? 'raise-exception (frame-procedure-name frame)))
                      (not (string-prefix? "ice-9/" (assq-ref loc 'file))))
                 (loop (+ i 1) frame))
                (else (loop (+ i 1) fallback)))))))

(define (condition-location obj)
  (let ((diagnostics (resolve-interface '(guix diagnostics))))
    (and ((module-ref diagnostics 'error-location?) obj)
         (location-alist ((module-ref diagnostics 'error-location) obj)))))

(define (condition-text obj)
  (let ((diagnostics (resolve-interface '(guix diagnostics))))
    (cond ((message-condition? obj) (condition-message obj))
          (((module-ref diagnostics 'formatted-message?) obj)
           (apply format #f
                  ((module-ref diagnostics 'formatted-message-string) obj)
                  ((module-ref diagnostics 'formatted-message-arguments) obj)))
          (else (format #f "exception thrown: ~s" obj)))))

(define (condition-hint obj)
  (let ((diagnostics (resolve-interface '(guix diagnostics))))
    (and ((module-ref diagnostics 'fix-hint?) obj)
         ((module-ref diagnostics 'condition-fix-hint) obj))))

(define (read-error-location message)
  ;; "FILE:LINE:COLUMN: rest" as Guile's reader reports it (1-based line,
  ;; 0-based column).
  (match (string-split message #\:)
    ((file line column . rest)
     (and (string->number line) (string->number column)
          `((file . ,file) (line . ,(string->number line))
            (column . ,(string->number column)))))
    (_ #f)))

(define (describe-error file args frame)
  "An error alist: message, optional location and hint."
  (define (with-location message loc hint)
    `((message . ,message)
      ,@(or loc (frame-location frame) '())
      ,@(if hint `((hint . ,hint)) '())))
  (match args
    (('read-error _ message . rest)
     (let ((text (apply format #f message rest)))
       (with-location text (read-error-location text) #f)))
    (('syntax-error proc message properties form subform . _)
     (let ((diagnostics (resolve-interface '(guix diagnostics))))
       (with-location (format #f "~s: ~a" (or subform form) message)
                      (and properties
                           (location-alist
                            ((module-ref diagnostics 'source-properties->location)
                             properties)))
                      #f)))
    (('unbound-variable _ message (variable) . _)
     (with-location (format #f message variable) #f
                    "Did you forget a use-modules form?"))
    (((or 'srfi-34 '%exception) obj)
     (with-location (condition-text obj) (condition-location obj)
                    (condition-hint obj)))
    (('system-error proc message rest . _)
     (with-location (format #f "~a: ~a" proc (apply format #f message rest)) #f #f))
    ((key proc message rest . _)
     (if (string? message)
         (with-location (format #f "~a: ~a" proc (apply format #f message (or rest '()))) #f #f)
         (with-location (format #f "~a: ~s" key args) #f #f)))
    (_ (with-location (format #f "~s" args) #f #f))))

(define (struct-name value)
  (and (struct? value) (struct-vtable-name (struct-vtable value))))

(define (fold-service-graph services target-type)
  "Fold the service graph as `guix system build` would; returns the root
service or throws."
  (let ((fold-services (module-ref (resolve-interface '(gnu services))
                                   'fold-services)))
    (fold-services services #:target-type target-type)))

(define (classify value)
  "Return (kind summary checks) for VALUE, running the checks that need no
store. Throws on a broken service graph."
  ;; Guix record accessors are macros: apply them through eval. Variables
  ;; such as service types are taken as they are.
  (define (ref module name)
    (let ((value (module-ref (resolve-module module) name)))
      (if (macro? value)
          (eval `(lambda args (apply ,name args)) (resolve-module module))
          value)))
  (case (struct-name value)
    ((<operating-system>)
     (let* ((services ((ref '(gnu system) 'operating-system-services) value))
            (host ((ref '(gnu system) 'operating-system-host-name) value)))
       (fold-service-graph services (ref '(gnu services) 'system-service-type))
       (list 'operating-system
             (format #f "operating system ~s with ~a services" host (length services))
             "loaded as `guix system` would; service graph folds; no derivation computed")))
    ((<home-environment>)
     (let ((services ((ref '(gnu home) 'home-environment-services) value)))
       (fold-service-graph services (ref '(gnu home) 'home-service-type))
       (list 'home-environment
             (format #f "home environment with ~a services" (length services))
             "loaded as `guix home` would; service graph folds; no derivation computed")))
    ((<package>)
     (list 'package
           (format #f "package ~a@~a"
                   ((ref '(guix packages) 'package-name) value)
                   ((ref '(guix packages) 'package-version) value))
           "loaded; package record constructed; no derivation computed"))
    ((<manifest>)
     (list 'manifest
           (format #f "manifest with ~a entries"
                   (length ((ref '(guix profiles) 'manifest-entries) value)))
           "loaded; manifest constructed; no derivation computed"))
    (else
     (list 'unknown
           (format #f "loaded; the file's value is ~a"
                   (or (struct-name value)
                       (if (unspecified? value) "unspecified"
                           (with-output-to-string (lambda () (write value))))))
           "loaded in a user module; not a system, home, package, or manifest"))))

(define (validate-file file)
  "Load FILE as Guix would and return the validation S-expression."
  (define tag (make-prompt-tag "user-code"))
  (define errors '())
  (define frame #f)
  (define (fail! err) (set! errors (cons err errors)))
  (define (load-in-user-module)
    (let ((module (make-user-module (append system-modules home-modules))))
      (save-module-excursion
       (lambda ()
         (set! %load-should-auto-compile #t)
         (set-current-module module)
         (parameterize ((current-warning-port (%make-void-port "w")))
           (load (canonicalize-path file)))))))
  (define (guarded thunk)
    (catch #t
      (lambda () (call-with-prompt tag thunk (const #f)))
      (lambda args (fail! (describe-error file args frame)) #f)
      (lambda args
        (let ((stack (make-stack #t 1 tag)))
          (set! frame (and stack (last-frame-with-source stack file)))))))
  (let* ((value (guarded load-in-user-module))
         (kind (and (null? errors) (guarded (lambda () (classify value))))))
    `(validation
      (file ,file)
      (kind ,(if kind (car kind) 'error))
      (summary ,(if kind (cadr kind) "failed to load"))
      (checks ,(if kind (caddr kind) ""))
      (errors ,(reverse errors)))))

(define (main args)
  (match args
    ((_ file)
     (write (validate-file file))
     (newline))
    (_ (format (current-error-port) "usage: validate.scm FILE~%")
       (exit 2))))
