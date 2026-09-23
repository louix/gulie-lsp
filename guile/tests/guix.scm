;;; Guix integration tests.  Run inside the selected Guix environment:
;;;   guix repl -L guile -L fixtures/guix -- guile/tests/guix.scm
(use-modules (srfi srfi-64) (srfi srfi-1) (guile-lsp metadata))

(define (field fields name)
  (or (find (lambda (f) (eq? (assq-ref f 'name) name)) fields)
      (error "field not reported" name)))

(test-begin "guix-metadata")
(define runner (test-runner-current))

(test-assert "define-configuration fields come from runtime metadata with docs and defaults"
  (let* ((result (guix-configuration-fields '(gnu services pm) 'tlp-configuration))
         (fields (assq-ref result 'fields))
         (mode (field fields 'tlp-default-mode))
         (governor (field fields 'cpu-scaling-governor-on-ac)))
    (and (eq? (assq-ref result 'source) 'runtime)
         (eq? (assq-ref mode 'type) 'string)
         (equal? (assq-ref mode 'default) '((value . "AC")))
         (string-prefix? "Default mode" (assq-ref mode 'documentation))
         (eq? (assq-ref governor 'type) 'maybe-space-separated-string-list))))

(test-assert "operating-system fields fall back to source extraction with defaults"
  (let* ((result (guix-configuration-fields '(gnu system) 'operating-system))
         (fields (assq-ref result 'fields)))
    (and (eq? (assq-ref result 'source) 'source-file)
         (assq-ref (field fields 'host-name) 'required?)
         (assq-ref (field fields 'bootloader) 'required?)
         (equal? (assq-ref (field fields 'timezone) 'default) "\"Etc/UTC\"")
         (assq-ref (field fields 'services) 'thunked?)
         (equal? (assq-ref (field fields 'services) 'sanitize) "validate-service-list")
         (assq-ref (field fields 'location) 'innate?))))

(test-assert "two define-record-type* service configurations are covered"
  (every (lambda (spec)
           (let* ((result (guix-configuration-fields (car spec) (cadr spec)))
                  (fields (assq-ref result 'fields)))
             (and (eq? (assq-ref result 'source) 'source-file)
                  (equal? (assq-ref (field fields (caddr spec)) 'default) (cadddr spec)))))
         '(((gnu services ssh) openssh-configuration port-number "22")
           ((gnu services sysctl) sysctl-configuration settings "%default-sysctl-settings"))))

(test-assert "nested OS records are covered"
  (every (lambda (spec)
           (let ((fields (assq-ref (guix-configuration-fields (car spec) (cadr spec)) 'fields)))
             (and fields (field fields (caddr spec)))))
         '(((gnu bootloader) bootloader-configuration targets)
           ((gnu system file-systems) file-system mount-point)
           ((gnu system accounts) user-account supplementary-groups))))

(test-assert "a local helper module works through both paths"
  (let ((hello (guix-configuration-fields '(my services) 'my-hello-configuration))
        (mirror (guix-configuration-fields '(my services) 'my-mirror)))
    (and (eq? (assq-ref hello 'source) 'runtime)
         (equal? (assq-ref (field (assq-ref hello 'fields) 'repeat) 'default) '((value . 1)))
         (eq? (assq-ref mirror 'source) 'source-file)
         (assq-ref (field (assq-ref mirror 'fields) 'url) 'required?))))

(test-assert "service types report description, default value type, and location"
  (let ((ssh (guix-service-type-summary '(gnu services ssh) 'openssh-service-type))
        (mine (guix-service-type-summary '(my services) 'my-hello-service-type)))
    (and (eq? (assq-ref ssh 'service-name) 'openssh)
         (string-prefix? "Run the OpenSSH" (assq-ref ssh 'description))
         (eq? (assq-ref ssh 'default-value) '<openssh-configuration>)
         (memq 'shepherd-root (assq-ref ssh 'extends))
         (string-suffix? "gnu/services/ssh.scm" (assq-ref (assq-ref ssh 'location) 'file))
         (eq? (assq-ref mine 'default-value) '<my-hello-configuration>)
         (string-suffix? "fixtures/guix/my/services.scm" (assq-ref (assq-ref mine 'location) 'file)))))

(test-assert "unknown records report an explicit unavailable fallback"
  (let ((result (guix-configuration-fields '(gnu services ssh) 'no-such-configuration)))
    (and (eq? (assq-ref result 'source) 'unavailable)
         (not (assq-ref result 'fields)))))

(test-assert "exports of a Guix module classify macros, variables, and procedures"
  (let* ((bindings (assq-ref (module-exports '(gnu services ssh)) 'bindings))
         (kind (lambda (name) (assq-ref (find (lambda (b) (eq? (assq-ref b 'name) name)) bindings) 'kind))))
    (and (eq? (kind 'openssh-configuration) 'macro)
         (eq? (kind 'openssh-service-type) 'variable))))

(test-assert "package and origin records are covered by the source adapter"
  (let* ((package (guix-configuration-fields '(guix packages) 'package))
         (origin (guix-configuration-fields '(guix packages) 'origin))
         (fields (assq-ref package 'fields)))
    (and (eq? (assq-ref package 'source) 'source-file)
         (assq-ref (field fields 'name) 'required?)
         (assq-ref (field fields 'build-system) 'required?)
         (equal? (assq-ref (field fields 'inputs) 'default) "(quote ())")
         (assq-ref (field fields 'inputs) 'thunked?)
         (field fields 'synopsis)
         (eq? (assq-ref origin 'source) 'source-file)
         (assq-ref (field (assq-ref origin 'fields) 'uri) 'required?)
         (field (assq-ref origin 'fields) 'method))))

(test-assert "configuration fields resolve through re-exporting modules"
  (let ((os (guix-configuration-fields '(gnu) 'operating-system))
        (tlp (guix-configuration-fields '(gnu services pm) 'tlp-configuration)))
    (and (eq? (assq-ref os 'source) 'source-file)
         (string-suffix? "gnu/system.scm" (assq-ref os 'file))
         (field (assq-ref os 'fields) 'host-name)
         (eq? (assq-ref tlp 'source) 'runtime))))

(test-assert "service type variables get a location through the interface chain"
  (let* ((module (resolve-interface '(my services)))
         (variable (module-variable module 'my-hello-service-type))
         (info (binding-info 'my-hello-service-type (variable-ref variable) #:module module #:variable variable))
         (location (assq-ref info 'location)))
    (and (string-suffix? "fixtures/guix/my/services.scm" (assq-ref location 'file))
         (= (assq-ref location 'line) 35))))

(test-assert "fields carry locations, including define-configuration and maybe-typed ones"
  (let* ((tlp (guix-configuration-fields '(gnu services pm) 'tlp-configuration))
         (boost (field (assq-ref tlp 'fields) 'cpu-boost-on-ac?))
         (ssh (guix-configuration-fields '(gnu services ssh) 'openssh-configuration))
         (port (field (assq-ref ssh 'fields) 'port-number)))
    (and (string-suffix? "gnu/services/pm.scm" (assq-ref tlp 'file))
         (string-suffix? "gnu/services/pm.scm" (assq-ref (assq-ref boost 'location) 'file))
         (eq? (assq-ref boost 'type) 'maybe-boolean)
         (= (assq-ref (assq-ref port 'location) 'line) 109))))

(test-assert "umbrella modules report re-exported bindings"
  (let ((names (map (lambda (b) (assq-ref b 'name)) (assq-ref (module-exports '(gnu)) 'bindings))))
    (and (memq 'operating-system names)
         (memq 'service names)
         (memq 'use-service-modules names)
         (> (length names) 500))))

(test-end "guix-metadata")
(exit (if (zero? (test-runner-fail-count runner)) 0 1))
