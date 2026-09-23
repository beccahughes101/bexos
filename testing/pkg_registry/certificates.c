/* Deterministic, test-only Ed25519 identities. Generated exclusively by Bazel. */
#include <openssl/evp.h>
#include <openssl/x509.h>
#include <openssl/x509v3.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
static void check(int ok) { if (!ok) { fputs("certificate fixture generation failed\n",stderr); exit(1); } }
static EVP_PKEY* key(unsigned char value) { unsigned char seed[32]; memset(seed,value,sizeof(seed)); EVP_PKEY* result=EVP_PKEY_new_raw_private_key(EVP_PKEY_ED25519,NULL,seed,sizeof(seed)); check(result!=NULL); return result; }
static void extension(X509* cert,X509* issuer,int nid,const char* value) { X509V3_CTX ctx; X509V3_set_ctx(&ctx,issuer,cert,NULL,NULL,0); X509_EXTENSION* ext=X509V3_EXT_conf_nid(NULL,&ctx,nid,(char*)value); check(ext!=NULL); check(X509_add_ext(cert,ext,-1)); X509_EXTENSION_free(ext); }
static X509* certificate(EVP_PKEY* subject,EVP_PKEY* signer,X509* issuer,int serial,const char* name,int ca) {
    X509* cert=X509_new(); check(cert!=NULL); check(X509_set_version(cert,2)); check(ASN1_INTEGER_set(X509_get_serialNumber(cert),serial));
    check(ASN1_TIME_set_string(X509_getm_notBefore(cert),"20200101000000Z")); check(ASN1_TIME_set_string(X509_getm_notAfter(cert),"20990101000000Z"));
    X509_NAME* subject_name=X509_get_subject_name(cert); check(X509_NAME_add_entry_by_txt(subject_name,"CN",MBSTRING_ASC,(const unsigned char*)name,-1,-1,0));
    check(X509_set_issuer_name(cert,issuer?X509_get_subject_name(issuer):subject_name)); check(X509_set_pubkey(cert,subject));
    extension(cert,issuer?issuer:cert,NID_basic_constraints,ca?"critical,CA:TRUE":"critical,CA:FALSE");
    extension(cert,issuer?issuer:cert,NID_key_usage,ca?"critical,keyCertSign,cRLSign":"critical,digitalSignature");
    if (!ca) { extension(cert,issuer,NID_ext_key_usage,"serverAuth,clientAuth"); extension(cert,issuer,NID_subject_alt_name,"DNS:registry.test,DNS:localhost,IP:127.0.0.1,IP:10.0.2.2"); }
    check(X509_sign(cert,signer,NULL)>0); return cert;
}
static FILE* output(const char* dir,const char* name) { char path[4096]; check(snprintf(path,sizeof(path),"%s/%s",dir,name)<(int)sizeof(path)); FILE* f=fopen(path,"wb"); check(f!=NULL); return f; }
static void write_cert(const char* dir,const char* name,X509* cert) { FILE* f=output(dir,name); check(i2d_X509_fp(f,cert)); check(fclose(f)==0); }
static void write_key(const char* dir,const char* name,EVP_PKEY* key) { FILE* f=output(dir,name); PKCS8_PRIV_KEY_INFO* info=EVP_PKEY2PKCS8(key); check(info!=NULL); check(i2d_PKCS8_PRIV_KEY_INFO_fp(f,info)); PKCS8_PRIV_KEY_INFO_free(info); check(fclose(f)==0); }
int main(int argc,char** argv) { check(argc==2); EVP_PKEY* root_key=key(41), *server_key=key(42), *client_key=key(43); X509* root=certificate(root_key,root_key,NULL,1,"RFC64 test root",1); X509* server=certificate(server_key,root_key,root,2,"RFC64 test server",0); X509* client=certificate(client_key,root_key,root,3,"RFC64 test client",0);
write_cert(argv[1],"root.der",root); write_cert(argv[1],"server.der",server); write_cert(argv[1],"client.der",client); write_key(argv[1],"server.key",server_key); write_key(argv[1],"client.key",client_key); X509_free(root); X509_free(server); X509_free(client); EVP_PKEY_free(root_key); EVP_PKEY_free(server_key); EVP_PKEY_free(client_key); return 0; }
