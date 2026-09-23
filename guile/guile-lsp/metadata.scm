;;; guile-lsp metadata extractor.
;;;
;;; Run inside the target Guile environment and print one S-expression
;;; describing the requested metadata.
;;;
;;;   guile -L guile -e '(guile-lsp metadata)' -s guile/guile-lsp/metadata.scm \
;;;         exports '(srfi srfi-1)'
;;;   guix repl -- guile/guile-lsp/metadata.scm configuration '(gnu services ssh)' openssh-configuration
;;;
;;; Commands:
;;;   exports MODULE            public bindings with kind, arguments, docs, source
;;;   source-records FILE       record definitions read from source without loading it
;;;   configuration MODULE NAME Guix configuration fields (runtime, then source fallback)
;;;   service-type MODULE NAME  Guix service-type summary
;;;
;;; Every command returns an alist.  Failures return ((error . MESSAGE)) with
;;; exit status 1.  Source lines and columns are 0-based, as Guile reports them.

(define-module (guile-lsp metadata)
  #:use-module (ice-9 match)
  #:use-module (ice-9 documentation)
  #:use-module (srfi srfi-1)
  #:use-module (system vm program)
  #:export (main
            binding-info
            module-exports
            source-records
            guix-configuration-fields
            guix-service-type-summary))


;;;
;;; Source locations.
;;;

(define (absolute-source-file file)
  "Resolve FILE, as recorded in compiled code, against the load path."
  (cond ((not file) #f)
        ((absolute-file-name? file) file)
        ((search-path %load-path file) => canonicalize-path)
        ((file-exists? file) (canonicalize-path file))
        (else file)))

(define (program-location program)
  (match (and (program? program) (program-source program 0))
    ((_ file line . column)
     `((file . ,(absolute-source-file file)) (line . ,line) (column . ,column)))
    (_ #f)))

(define (properties-location props)
  (let ((file (assq-ref props 'filename))
        (line (assq-ref props 'line))
        (column (assq-ref props 'column)))
    (and line
         `((file . ,(absolute-source-file file)) (line . ,line) (column . ,column)))))


;;;
;;; Bindings.
;;;

(define (arguments-alist->info alist)
  `((required . ,(assq-ref alist 'required))
    (optional . ,(assq-ref alist 'optional))
    (keyword . ,(map (lambda (k) (keyword->symbol (car k)))
                     (or (assq-ref alist 'keyword) '())))
    (allow-other-keys? . ,(assq-ref alist 'allow-other-keys?))
    (rest . ,(assq-ref alist 'rest))))

(define (procedure-arguments-info proc)
  "Describe PROC's arguments, preferring the compiler's lambda list. The
first clause of a case-lambda; see procedure-arities for all of them."
  (cond ((program? proc)
         (let ((alist (program-arguments-alist proc)))
           (if alist
               (arguments-alist->info alist)
               (minimum-arity-info proc))))
        (else (minimum-arity-info proc))))

(define (procedure-arities proc)
  "Every clause of PROC (one for plain lambdas, several for case-lambda),
or #f when only the minimum arity is known."
  (and (program? proc)
       (let ((alists (program-arguments-alists proc)))
         (and (pair? alists) (map arguments-alist->info alists)))))

(define (minimum-arity-info proc)
  (match (procedure-minimum-arity proc)
    ((required optional rest?)
     `((required-count . ,required) (optional-count . ,optional) (rest? . ,rest?)))
    (_ #f)))

(define (value-kind value)
  (cond ((macro? value) 'macro)
        ((procedure? value) 'procedure)
        ((and (struct? value) (record-type? value)) 'record-type)
        (else 'variable)))

(define (value-type-name value)
  (cond ((string? value) 'string)
        ((symbol? value) 'symbol)
        ((keyword? value) 'keyword)
        ((boolean? value) 'boolean)
        ((exact-integer? value) 'integer)
        ((number? value) 'number)
        ((null? value) 'null)
        ((pair? value) 'list)
        ((vector? value) 'vector)
        ((char? value) 'char)
        ((hash-table? value) 'hash-table)
        ;; Not every struct is a record instance: vtables and other raw
        ;; structs live in (guile) too.
        ((struct? value)
         (let ((vtable (struct-vtable value)))
           (if (record-type? vtable) (record-type-name vtable) 'struct)))
        (else 'unknown)))

(define* (binding-info name value #:key module variable)
  "Describe VALUE bound to NAME. MODULE and VARIABLE, when given, let
variables and record types get a source location by reading their file."
  (define (source-location)
    (and module variable (variable-location module name variable)))
  (case (value-kind value)
    ((procedure)
     `((name . ,name)
       (kind . procedure)
       (arguments . ,(procedure-arguments-info value))
       (arities . ,(procedure-arities value))
       (documentation . ,(procedure-documentation value))
       (location . ,(program-location value))))
    ((macro)
     (let ((transformer (macro-transformer value)))
       `((name . ,name)
         (kind . macro)
         (documentation . ,(and (procedure? transformer)
                                (procedure-documentation transformer)))
         ;; The transformer's location is the define-syntax form itself.
         (location . ,(program-location transformer)))))
    ((record-type)
     `((name . ,name)
       (kind . record-type)
       (type-name . ,(record-type-name value))
       (fields . ,(record-type-fields value))
       (location . ,(source-location))))
    (else
     `((name . ,name)
       (kind . variable)
       (type . ,(value-type-name value))
       (location . ,(source-location))))))

(define (interface-bindings interface)
  "Collect (symbol . variable) pairs visible through INTERFACE, including
re-exported interfaces such as those aggregated by (gnu).  The first
definition seen wins, matching Guile's own lookup order."
  (let ((seen (make-hash-table))
        (result '()))
    (let walk ((interface interface))
      (module-for-each (lambda (symbol variable)
                         (unless (hashq-ref seen symbol)
                           (hashq-set! seen symbol #t)
                           (set! result (cons (cons symbol variable) result))))
                       interface)
      (for-each walk (module-uses interface)))
    result))

(define* (module-exports name #:key (private? #f))
  "Describe the public bindings of module NAME, or all bindings with PRIVATE?."
  (let* ((module (resolve-module name #:ensure #f))
         (interface (and module (if private? module (module-public-interface module)))))
    (unless module
      (error "module not found" name))
    `((module . ,name)
      (file . ,(absolute-source-file (module-filename module)))
      (imports . ,(delete-duplicates (map module-name (module-uses module))))
      (bindings . ,(sort (map (match-lambda
                                ((symbol . variable)
                                 (if (variable-bound? variable)
                                     (binding-info symbol (variable-ref variable))
                                     `((name . ,symbol) (kind . unbound)))))
                              (if private?
                                  (module-map cons interface)
                                  (interface-bindings interface)))
                         (lambda (a b)
                           (string<? (symbol->string (assq-ref a 'name))
                                     (symbol->string (assq-ref b 'name)))))))))


;;;
;;; Locations for variables: Guile keeps none, so read the defining file.
;;;

(define (variable-home-module module name variable)
  "The module whose own table holds VARIABLE, searching MODULE and the
interfaces it uses, or #f."
  (let ((seen (make-hash-table)))
    (let walk ((module module))
      (and module
           (not (hashq-ref seen module))
           (begin
             (hashq-set! seen module #t)
             (let ((local (module-local-variable module name)))
               (if (and local (eq? local variable))
                   module
                   (any walk (module-uses module)))))))))

(define definition-heads
  '(define define* define-public define-once define-inlinable define-syntax
    define-syntax-rule define-record-type define-record-type* define-values
    define-immutable-record-type define-configuration define-macro))

(define (definition-target form)
  "The symbol a top-level definition FORM binds, or #f."
  (match form
    (((? symbol? head) target . _)
     (and (memq head definition-heads)
          (let loop ((target target))
            (cond ((symbol? target) target)
                  ((pair? target) (loop (car target)))
                  (else #f)))))
    (_ #f)))

(define (source-definition-location file name)
  "Location of the top-level form defining NAME in FILE, by reading it."
  (and file (file-exists? file)
       (any (lambda (form)
              (and (eq? (definition-target form) name)
                   (properties-location (source-properties form))))
            (read-forms file))))

(define (variable-location module name variable)
  (let* ((home (variable-home-module module name variable))
         ;; A public interface holds the same variables but no filename;
         ;; the module it belongs to has the same name.
         (home (and home (if (module-filename home)
                             home
                             (resolve-module (module-name home) #:ensure #f))))
         (file (and home (absolute-source-file (module-filename home)))))
    (source-definition-location file name)))

;;;
;;; Source-only record extraction: no code is loaded or evaluated.
;;;

(define (install-tolerant-reader!)
  "Accept Guix gexp reader syntax (#~ #$ #$@ #+ #+@) without loading Guix.
The wrapped datum is tagged so source extraction can still walk it."
  (define (extension tag)
    (lambda (char port)
      (if (eqv? (peek-char port) #\@)
          (begin (read-char port) (list (symbol-append tag '-splicing) (read port)))
          (list tag (read port)))))
  (read-hash-extend #\~ (extension 'gexp))
  (read-hash-extend #\$ (extension 'ungexp))
  (read-hash-extend #\+ (extension 'ungexp-native)))

(define (read-forms file)
  (install-tolerant-reader!)
  (call-with-input-file file
    (lambda (port)
      (read-enable 'positions)
      (let loop ((forms '()))
        (let ((form (catch #t
                      (lambda () (read port))
                      (lambda (key . args)
                        ;; Stop at the first unreadable form; return what we have.
                        (cons 'guile-lsp-read-error args)))))
          (cond ((eof-object? form) (reverse forms))
                ((and (pair? form) (eq? (car form) 'guile-lsp-read-error))
                 (reverse (cons form forms)))
                (else (loop (cons form forms)))))))))

(define (datum->string datum)
  (call-with-output-string (lambda (port) (write datum port))))

(define (form-location form)
  (properties-location (source-properties form)))

(define (star-field field)
  "Parse a define-record-type* field spec into a field alist."
  (match field
    (((? symbol? name) (? symbol? accessor) properties ...)
     (let ((default (assq 'default properties)))
       `((name . ,name)
         (location . ,(form-location field))
         (accessor . ,accessor)
         (required? . ,(not default))
         (default . ,(and default (datum->string (cadr default))))
         (thunked? . ,(and (assq 'thunked properties) #t))
         (delayed? . ,(and (assq 'delayed properties) #t))
         (innate? . ,(and (assq 'innate properties) #t))
         (sanitize . ,(and=> (assq 'sanitize properties)
                             (lambda (s) (datum->string (cadr s))))))))
    (_ `((name . ,(datum->string field)) (unparsed? . #t)))))

(define (srfi-9-field field)
  (match field
    (((? symbol? name) (? symbol? accessor) rest ...)
     `((name . ,name) (accessor . ,accessor) (location . ,(form-location field))
       (modifier . ,(and (pair? rest) (car rest)))))
    ((? symbol? name) `((name . ,name)))
    (_ `((name . ,(datum->string field)) (unparsed? . #t)))))

(define (configuration-field field)
  "Parse a define-configuration field: (name (type default?) doc? serializer?)."
  (match field
    (((? symbol? name) (type . default) (? string? doc) . _)
     `((name . ,name) (type . ,type) (location . ,(form-location field))
       (required? . ,(null? default))
       (default . ,(and (pair? default) (datum->string (car default))))
       (documentation . ,doc)))
    (((? symbol? name) (type . default) . _)
     `((name . ,name) (type . ,type) (location . ,(form-location field))
       (required? . ,(null? default))
       (default . ,(and (pair? default) (datum->string (car default))))))
    ;; `(name maybe-type "doc")`: a bare type means the field is optional and
    ;; unset by default (Guix's maybe- types), or required otherwise.
    (((? symbol? name) (? symbol? type) . rest)
     `((name . ,name) (type . ,type) (location . ,(form-location field))
       (required? . ,(not (string-prefix? "maybe-" (symbol->string type))))
       (default . #f)
       (documentation . ,(and (pair? rest) (string? (car rest)) (car rest)))))
    (_ #f)))

(define (source-record form)
  (match form
    (('define-record-type* type constructor _ predicate (? symbol? this) fields ...)
     `((kind . guix-record) (type . ,type) (constructor . ,constructor)
       (this . ,this) (location . ,(form-location form))
       (fields . ,(map star-field fields))))
    (('define-record-type* type constructor _ predicate fields ...)
     `((kind . guix-record) (type . ,type) (constructor . ,constructor)
       (location . ,(form-location form))
       (fields . ,(map star-field fields))))
    (((or 'define-record-type 'define-immutable-record-type) type (constructor . args) _ fields ...)
     `((kind . srfi-9-record) (type . ,type) (constructor . ,constructor)
       (constructor-arguments . ,args) (location . ,(form-location form))
       (fields . ,(map srfi-9-field fields))))
    (('define-configuration name fields ...)
     `((kind . guix-configuration) (type . ,name)
       (location . ,(form-location form))
       (fields . ,(filter-map configuration-field fields))))
    (_ #f)))

(define (source-records file)
  "Return record definitions found by reading FILE, without loading it."
  (let ((forms (read-forms file)))
    `((file . ,file)
      (read-error . ,(and=> (find (lambda (f) (and (pair? f) (eq? (car f) 'guile-lsp-read-error))) forms)
                            (lambda (e) (datum->string (cdr e)))))
      (records . ,(filter-map source-record forms)))))


;;;
;;; Guix adapter.  Only active when the Guix modules resolve in this Guile.
;;;

(define (module-procedure module symbol)
  "Return a procedure applying SYMBOL in MODULE.  Guix record accessors are
inlinable macros, so module-ref alone would return a syntax transformer."
  (eval `(lambda args (apply ,symbol args)) module))

(define (guix-module symbols . name)
  (let ((module (resolve-module name #:ensure #f)))
    (and module
         (every (lambda (s) (module-variable module s)) symbols)
         module)))

(define (configuration-module)
  (guix-module '(configuration-field-name configuration-field-type
                 configuration-field-default-value-thunk
                 configuration-field-documentation)
               'gnu 'services 'configuration))

(define (safe-default thunk)
  "Evaluate a field default thunk; report opaque values by type only."
  (catch #t
    (lambda ()
      (let ((value (thunk)))
        (if (or (string? value) (number? value) (boolean? value)
                (symbol? value) (null? value) (keyword? value))
            `((value . ,value))
            `((opaque . ,(value-type-name value))))))
    (lambda (key . args)
      `((unavailable . ,(datum->string (cons key args)))))))

(define (runtime-configuration-fields module name)
  (let* ((config (configuration-module))
         (ref (lambda (s) (module-procedure config s)))
         (variable (module-variable module (symbol-append name '-fields)))
         ;; Runtime metadata has no positions: pair it with the source read.
         (file (absolute-source-file (module-filename module)))
         (source-fields (or (and file
                                 (and=> (find (lambda (r) (eq? (assq-ref r 'type) name))
                                              (assq-ref (source-records file) 'records))
                                        (lambda (r) (assq-ref r 'fields))))
                            '())))
    (and config variable
         (map (lambda (field)
                `((name . ,((ref 'configuration-field-name) field))
                  (location . ,(and=> (find (lambda (f) (eq? (assq-ref f 'name) ((ref 'configuration-field-name) field)))
                                            source-fields)
                                      (lambda (f) (assq-ref f 'location))))
                  (type . ,((ref 'configuration-field-type) field))
                  (default . ,(let ((thunk ((ref 'configuration-field-default-value-thunk) field)))
                                (if thunk (safe-default thunk) '((required . #t)))))
                  (documentation . ,((ref 'configuration-field-documentation) field))))
              (variable-ref variable)))))

(define (binding-source-file module name)
  "The file defining NAME as seen through MODULE, from compiled metadata.
Umbrella modules such as (gnu) only see re-exports through their interface."
  (let ((variable (or (module-variable (module-public-interface module) name)
                      (module-variable module name))))
    (and variable (variable-bound? variable)
         (let* ((value (variable-ref variable))
                (program (cond ((macro? value) (macro-transformer value))
                               ((procedure? value) value)
                               (else #f))))
           ;; The raw source name is relative to a load-path entry, which is
           ;; exactly the module path; canonical paths would not match the
           ;; symlinked module unions Guix builds.
           (and program
                (match (and (program? program) (program-source program 0))
                  ((_ file . _) file)
                  (_ #f)))))))

(define (file->module-name file)
  "The module name implied by FILE: relative names map directly, absolute
names must sit under a %load-path entry."
  (and file
       (string-suffix? ".scm" file)
       (if (not (absolute-file-name? file))
           (map string->symbol
                (string-split (substring file 0 (- (string-length file) 4)) #\/))
       (any (lambda (dir)
              (let ((dir (if (string-suffix? "/" dir) dir (string-append dir "/"))))
                (and (string-prefix? dir file)
                     (string-suffix? ".scm" file)
                     (map string->symbol
                          (string-split (substring file (string-length dir)
                                                   (- (string-length file) 4))
                                        #\/)))))
            (map (lambda (dir) (or (false-if-exception (canonicalize-path dir)) dir))
                 %load-path)))))

(define (defining-module module name)
  "The module whose file defines NAME, when it can be found and is loaded;
otherwise MODULE itself. Re-exporting umbrellas such as (gnu) do not hold
the private `NAME-fields` metadata, their sources do."
  (let* ((file (binding-source-file module name))
         (module-name (file->module-name file)))
    (or (and module-name (resolve-module module-name #:ensure #f))
        module)))

(define (with-field-aliases name fields)
  "Syntactic sugar some constructor macros add over their record: `origin`
accepts `sha256` in place of `hash`."
  (define aliases
    (case name
      ((origin) '((hash sha256)))
      (else '())))
  (if (and (pair? fields) (pair? aliases))
      (map (lambda (field)
             (match (assq (assq-ref field 'name) aliases)
               ((_ . names) (cons (cons 'aliases names) field))
               (#f field)))
           fields)
      fields))

(define (guix-configuration-fields module-name name)
  "Describe the fields of configuration record NAME visible through MODULE-NAME."
  (let ((visible (resolve-module module-name #:ensure #f)))
    (unless visible (error "module not found" module-name))
    (let* ((module (defining-module visible name))
           (runtime (runtime-configuration-fields module name)))
      (if runtime
          `((module . ,module-name) (name . ,name) (source . runtime)
            (file . ,(absolute-source-file (module-filename module)))
            (fields . ,(with-field-aliases name runtime)))
          (let* ((file (absolute-source-file (module-filename module)))
                 (records (and file (assq-ref (source-records file) 'records)))
                 (record (and records
                              (find (lambda (r)
                                      (or (eq? (assq-ref r 'constructor) name)
                                          (eq? (assq-ref r 'type) name)
                                          ;; `origin` wraps `%origin`: accept the type name too.
                                          (eq? (assq-ref r 'type)
                                               (symbol-append '< name '>))))
                                    records))))
            (if record
                `((module . ,module-name) (name . ,name) (source . source-file)
                  (file . ,file) (fields . ,(with-field-aliases name (assq-ref record 'fields))))
                `((module . ,module-name) (name . ,name) (source . unavailable)
                  (fields . #f))))))))

(define (guix-service-type-summary module-name name)
  (let* ((services (guix-module '(service-type-name service-type-description
                                  service-type-default-value service-type-extensions
                                  service-extension-target service-type-location)
                                'gnu 'services))
         (module (resolve-module module-name #:ensure #f)))
    (unless services (error "Guix (gnu services) is not available in this Guile"))
    (unless module (error "module not found" module-name))
    (let* ((ref (lambda (s) (module-procedure services s)))
           (diagnostics (resolve-module '(guix diagnostics)))
           (location-part (lambda (s) (module-procedure diagnostics s)))
           (type (module-ref module name))
           (default ((ref 'service-type-default-value) type))
           (location ((ref 'service-type-location) type)))
      `((module . ,module-name)
        (name . ,name)
        (service-name . ,((ref 'service-type-name) type))
        (description . ,((ref 'service-type-description) type))
        (default-value . ,(cond ((unspecified? default) 'none)
                                ((struct? default) (record-type-name (struct-vtable default)))
                                (else (value-type-name default))))
        (extends . ,(map (lambda (extension)
                           ((ref 'service-type-name) ((ref 'service-extension-target) extension)))
                         ((ref 'service-type-extensions) type)))
        (location . ,(and location
                          `((file . ,(absolute-source-file ((location-part 'location-file) location)))
                            (line . ,(- ((location-part 'location-line) location) 1))
                            (column . ,((location-part 'location-column) location)))))))))


;;;
;;; Entry point.
;;;

(define (parse-module-name string)
  (let ((datum (call-with-input-string string read)))
    (if (list? datum) datum (error "expected a module name like (srfi srfi-1)" string))))

(define (run command args)
  (match (cons command args)
    (("exports" module) (module-exports (parse-module-name module)))
    (("exports" module "--private") (module-exports (parse-module-name module) #:private? #t))
    (("source-records" file) (source-records file))
    (("configuration" module name)
     (guix-configuration-fields (parse-module-name module) (string->symbol name)))
    (("service-type" module name)
     (guix-service-type-summary (parse-module-name module) (string->symbol name)))
    (_ (error "usage: exports MODULE [--private] | source-records FILE | configuration MODULE NAME | service-type MODULE NAME"))))

(define (main args)
  (let ((start (get-internal-real-time)))
    (match args
      ((_ command rest ...)
       (let ((result (catch #t
                       (lambda () (run command rest))
                       (lambda (key . error-args)
                         `((error . ,(datum->string (cons key error-args))))))))
         (write `(,@result
                  (elapsed-ms . ,(round (/ (* 1000 (- (get-internal-real-time) start))
                                          internal-time-units-per-second)))))
         (newline)
         (exit (if (assq 'error result) 1 0))))
      (_ (run "help" '())))))
