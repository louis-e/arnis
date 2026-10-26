scoreboard players add #t arnis.ds 1
execute if score #kind arnis.ds matches 3 run function arnis:disaster/hail/second
execute if score #kind arnis.ds matches 4 run function arnis:disaster/sinkhole/second
execute if score #kind arnis.ds matches 6 run function arnis:disaster/quake/second
execute if score #kind arnis.ds matches 7 run function arnis:disaster/flood/second
execute if score #kind arnis.ds matches 9 run function arnis:disaster/tsunami/second
execute if score #kind arnis.ds matches 10 run function arnis:disaster/kaiju/second
execute if score #kind arnis.ds matches 11 run function arnis:disaster/snow/second
# A disaster without a set length ends itself.
execute if score #len arnis.ds matches 1.. if score #t arnis.ds >= #len arnis.ds run scoreboard players set #done arnis.ds 1
execute if score #done arnis.ds matches 1 run function arnis:disaster/end
