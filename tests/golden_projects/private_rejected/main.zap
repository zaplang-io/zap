# Accessing a private member through an import is a compile error (ZAP3012).
import a

def main():
    print(a._secret())
