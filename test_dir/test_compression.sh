#!/bin/bash
set -e
export PATH=../target/release:$PATH

echo "======================================================"
echo "1. GENERANDO ARCHIVO DE PRUEBA (5MB)"
echo "======================================================"
# Generamos un archivo grande para asegurar que la compresión y el chunking funcionen a gran escala
head -c 5242880 /dev/urandom | base64 > original_data.txt
ls -lh original_data.txt

echo -e "\n======================================================"
echo "2. AUTENTICANDO USUARIO PRO"
echo "======================================================"
dedup-engine auth TEST-KEY

echo -e "\n======================================================"
echo "3. BACKUP CON COMPRESIÓN ZSTD (--compress)"
echo "======================================================"
dedup-engine backup original_data.txt --compress

echo -e "\n======================================================"
echo "4. RESTAURANDO (Descompresión al vuelo)"
echo "======================================================"
dedup-engine restore original_data.txt restored_data.txt

echo -e "\n======================================================"
echo "5. VERIFICACIÓN DE INTEGRIDAD BYTE POR BYTE"
echo "======================================================"
if cmp -s original_data.txt restored_data.txt; then
    echo "✅ ÉXITO TOTAL: ¡El archivo restaurado es EXACTAMENTE igual al original!"
    echo "Prueba de Hash MD5:"
    md5sum original_data.txt restored_data.txt
else
    echo "❌ ERROR: Los archivos son diferentes. La descompresión falló."
    exit 1
fi

# Limpieza
rm original_data.txt restored_data.txt
