# Consumer that instantiates `datekit`'s generic across the package boundary:
# once with a Date (the arm matches and binds `day`) and twice with scalars (the
# impossible arm is elided, the fallback raises, the wrapper reports "rejected").
import datekit
import datetime

def main():
    auto d = datekit.promote(datetime.date(2026, 11, 23))
    print(d.year)  # 2026
    print(d.day)  # 23
    print(datekit.reject(1))  # rejected
    print(datekit.reject(3.5))  # rejected
