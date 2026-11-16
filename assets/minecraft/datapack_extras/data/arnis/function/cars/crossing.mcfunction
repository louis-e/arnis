# A level crossing's stop line is red from when its lamps start to flash until its booms
# are up again: any state but 0 (see xing/check).
$execute if score #x$(lx) arnis.xing matches 1.. run scoreboard players set #red arnis.car 1
execute if score #red arnis.car matches 1 run function arnis:cars/ease with entity @s data
