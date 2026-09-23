#!/bin/sh
printf '%s\n' '{"respCode":0,"respData":[{"httpMethod":"post","httpPath":"/api/query","httpToScfFilterConfig":{"interfaceName":"p.IFacade","methodSignature":"query(QueryReq)"}},{"httpMethod":"POST","httpPath":"","httpToScfFilterConfig":{"interfaceName":"p.IFacade","methodSignature":"internal()"}}]}'
