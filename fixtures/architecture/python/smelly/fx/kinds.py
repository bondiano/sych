from fx import sockets


class Stream:
    pass


def fits(n: int) -> bool:
    return n < sockets.LIMIT
